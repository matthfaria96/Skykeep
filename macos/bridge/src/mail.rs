//! iCloud Mail, the way every mail app reaches it: IMAP at imap.mail.me.com to read, flag,
//! move and delete; SMTP at smtp.mail.me.com to send. Apple asks third-party mail apps for an
//! app-specific password (made once at account.apple.com); it is kept in the Mac's keychain.
//! Sent mail is filed in "Sent Messages", as Mail does, so it shows on every device.

use anyhow::{anyhow, Context, Result};
use async_imap::types::Flag;
use async_imap::Session;
use base64::Engine;
use futures_util::TryStreamExt;
use mail_parser::{MessageParser, MimeHeaders};
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::net::TcpStream;
use tokio_rustls::client::TlsStream;
use tokio_rustls::rustls::{pki_types::ServerName, ClientConfig, RootCertStore};
use tokio_rustls::TlsConnector;

const IMAP_HOST: &str = "imap.mail.me.com";
const SMTP_HOST: &str = "smtp.mail.me.com";
const KEYCHAIN_SERVICE: &str = "Skykeep Mail (iCloud)";
pub const TRASH: &str = "Deleted Messages";
pub const SENT: &str = "Sent Messages";

type Imap = Session<TlsStream<TcpStream>>;

pub struct Mail {
    email: Option<String>,
    session: Option<Imap>,
    selected: Option<String>,
}

fn config_path() -> PathBuf {
    crate::state_dir().join("mail.json")
}

impl Mail {
    pub fn load() -> Self {
        let email = std::fs::read(config_path())
            .ok()
            .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
            .and_then(|v| {
                v.get("email")
                    .and_then(Value::as_str)
                    .map(ToString::to_string)
            });
        Self {
            email,
            session: None,
            selected: None,
        }
    }

    fn password(email: &str) -> Result<String> {
        keyring::Entry::new(KEYCHAIN_SERVICE, email)
            .and_then(|e| e.get_password())
            .map_err(|_| anyhow!("A senha do e-mail não está mais no Chaveiro. Entre de novo."))
    }

    async fn connect(email: &str, password: &str) -> Result<Imap> {
        let mut roots = RootCertStore::empty();
        roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
        let config = ClientConfig::builder()
            .with_root_certificates(roots)
            .with_no_client_auth();
        let tcp = TcpStream::connect((IMAP_HOST, 993))
            .await
            .context("Sem conexão com o iCloud Mail.")?;
        let tls = TlsConnector::from(Arc::new(config))
            .connect(ServerName::try_from(IMAP_HOST)?.to_owned(), tcp)
            .await
            .context("Não deu para abrir uma conexão segura com o iCloud Mail.")?;
        let mut client = async_imap::Client::new(tls);
        let _greeting = client.read_response().await;
        client.login(email, password).await.map_err(|(e, _)| {
            let s = e.to_string();
            if s.to_lowercase().contains("auth") || s.contains("NO") {
                anyhow!("O iCloud recusou o e-mail ou a senha de app. Confira e tente de novo.")
            } else {
                anyhow!(s)
            }
        })
    }

    async fn session(&mut self) -> Result<&mut Imap> {
        if self.session.is_none() {
            let email = self
                .email
                .clone()
                .ok_or_else(|| anyhow!("Configure o e-mail do iCloud primeiro."))?;
            let password = Self::password(&email)?;
            self.session = Some(Self::connect(&email, &password).await?);
            self.selected = None;
        }
        Ok(self.session.as_mut().expect("just set"))
    }

    async fn select(&mut self, mailbox: &str) -> Result<u32> {
        let s = self.session().await?;
        let mb = s.select(mailbox).await?;
        self.selected = Some(mailbox.to_string());
        Ok(mb.exists)
    }

    pub fn status(&self) -> Value {
        json!({ "configured": self.email.is_some(), "email": self.email })
    }

    pub async fn setup(&mut self, email: &str, password: &str) -> Result<Value> {
        let email = email.trim().to_lowercase();
        let password: String = password.chars().filter(|c| !c.is_whitespace()).collect();
        let mut s = Self::connect(&email, &password).await?;
        let _ = s.logout().await;
        keyring::Entry::new(KEYCHAIN_SERVICE, &email)
            .and_then(|e| e.set_password(&password))
            .map_err(|e| anyhow!("Não deu para guardar a senha no Chaveiro: {e}"))?;
        std::fs::write(config_path(), json!({ "email": email }).to_string())?;
        self.email = Some(email);
        self.session = None;
        Ok(self.status())
    }

    pub async fn sign_out(&mut self) -> Result<Value> {
        if let Some(mut s) = self.session.take() {
            let _ = s.logout().await;
        }
        if let Some(email) = self.email.take() {
            if let Ok(e) = keyring::Entry::new(KEYCHAIN_SERVICE, &email) {
                let _ = e.delete_credential();
            }
        }
        let _ = std::fs::remove_file(config_path());
        Ok(self.status())
    }

    /// Runs `op`, and once more on a fresh connection if the first one had gone stale.
    async fn retry<T, F>(&mut self, mut op: F) -> Result<T>
    where
        F: for<'a> FnMut(
            &'a mut Mail,
        )
            -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<T>> + 'a>>,
    {
        match op(self).await {
            Ok(v) => Ok(v),
            Err(_) => {
                self.session = None;
                self.selected = None;
                op(self).await
            }
        }
    }

    /// The newest `limit` messages of a mailbox, each with its first lines for a preview.
    pub async fn list(&mut self, mailbox: String, limit: u32) -> Result<Value> {
        self.retry(move |m| {
            let mailbox = mailbox.clone();
            Box::pin(async move { m.list_once(&mailbox, limit).await })
        })
        .await
    }

    async fn list_once(&mut self, mailbox: &str, limit: u32) -> Result<Value> {
        let exists = self.select(mailbox).await?;
        let s = self.session().await?;
        let unseen = s
            .uid_search("UNSEEN")
            .await
            .map(|set| set.len())
            .unwrap_or(0);
        if exists == 0 {
            return Ok(json!({ "messages": [], "unread": unseen, "mailbox": mailbox }));
        }
        let first = exists.saturating_sub(limit.max(1)) + 1;
        let fetches: Vec<_> = s
            .fetch(
                format!("{first}:{exists}"),
                "(UID FLAGS INTERNALDATE BODY.PEEK[]<0.8000>)",
            )
            .await?
            .try_collect()
            .await?;
        let parser = MessageParser::default();
        let mut out: Vec<Value> = Vec::new();
        for f in &fetches {
            let Some(uid) = f.uid else { continue };
            let flags: Vec<Flag<'_>> = f.flags().collect();
            let seen = flags.iter().any(|x| matches!(x, Flag::Seen));
            let flagged = flags.iter().any(|x| matches!(x, Flag::Flagged));
            let raw = f.body().unwrap_or_default();
            let parsed = parser.parse(raw);
            let (name, address) = parsed
                .as_ref()
                .and_then(|p| p.from())
                .and_then(|a| a.first())
                .map(|a| {
                    (
                        a.name().unwrap_or_default().to_string(),
                        a.address().unwrap_or_default().to_string(),
                    )
                })
                .unwrap_or_default();
            let subject = parsed
                .as_ref()
                .and_then(|p| p.subject())
                .unwrap_or_default()
                .to_string();
            let date = parsed
                .as_ref()
                .and_then(|p| p.date())
                .map(|d| d.to_timestamp())
                .or_else(|| f.internal_date().map(|d| d.timestamp()))
                .unwrap_or(0);
            let preview = parsed
                .as_ref()
                .and_then(|p| {
                    p.body_text(0)
                        .map(|t| t.to_string())
                        .or_else(|| p.body_html(0).map(|h| strip_tags(&h)))
                })
                .map(|t| preview_of(&t))
                .unwrap_or_default();
            out.push(json!({
                "uid": uid, "seen": seen, "flagged": flagged,
                "fromName": if name.is_empty() { address.clone() } else { name }, "fromAddress": address,
                "subject": subject, "date": date, "preview": preview,
            }));
        }
        out.sort_by(|a, b| b["uid"].as_u64().cmp(&a["uid"].as_u64()));
        Ok(json!({ "messages": out, "unread": unseen, "mailbox": mailbox }))
    }

    /// One whole message: its HTML (pictures sent inside it shown in place) or text, who it
    /// went to, and what is attached.
    pub async fn body(&mut self, mailbox: String, uid: u32) -> Result<Value> {
        self.retry(move |m| {
            let mailbox = mailbox.clone();
            Box::pin(async move { m.body_once(&mailbox, uid).await })
        })
        .await
    }

    async fn body_once(&mut self, mailbox: &str, uid: u32) -> Result<Value> {
        self.select(mailbox).await?;
        let s = self.session().await?;
        let fetches: Vec<_> = s
            .uid_fetch(uid.to_string(), "(UID BODY.PEEK[])")
            .await?
            .try_collect()
            .await?;
        let raw = fetches
            .iter()
            .find_map(|f| f.body())
            .ok_or_else(|| anyhow!("Essa mensagem não está mais no servidor."))?;
        let msg = MessageParser::default()
            .parse(raw)
            .ok_or_else(|| anyhow!("Não deu para ler essa mensagem."))?;
        let mut html = msg.body_html(0).map(|h| h.to_string());
        let text = msg.body_text(0).map(|t| t.to_string());
        let mut attachments: Vec<String> = Vec::new();
        for part in msg.attachments() {
            let ctype = part
                .content_type()
                .map(|c| format!("{}/{}", c.ctype(), c.subtype().unwrap_or("octet-stream")))
                .unwrap_or_default();
            if let (Some(cid), Some(h)) = (part.content_id(), html.as_mut()) {
                let data = base64::engine::general_purpose::STANDARD.encode(part.contents());
                let cid = cid.trim_matches(|c| c == '<' || c == '>');
                *h = h.replace(
                    &format!("cid:{cid}"),
                    &format!("data:{ctype};base64,{data}"),
                );
                continue;
            }
            attachments.push(part.attachment_name().unwrap_or("Anexo").to_string());
        }
        let list = |a: Option<&mail_parser::Address<'_>>| -> Vec<String> {
            a.map(|a| {
                a.iter()
                    .filter_map(|x| x.address().map(ToString::to_string))
                    .collect()
            })
            .unwrap_or_default()
        };
        Ok(json!({
            "html": html, "text": text, "attachments": attachments,
            "messageId": msg.message_id(),
            "replyTo": list(msg.reply_to()).first().cloned().or_else(|| list(msg.from()).first().cloned()),
            "to": list(msg.to()), "cc": list(msg.cc()),
        }))
    }

    /// Read / unread, flagged / not — on the server, so every device agrees.
    pub async fn flag(
        &mut self,
        mailbox: String,
        uid: u32,
        seen: Option<bool>,
        flagged: Option<bool>,
    ) -> Result<Value> {
        self.retry(move |m| {
            let mailbox = mailbox.clone();
            Box::pin(async move {
                m.select(&mailbox).await?;
                let s = m.session().await?;
                for (on, name) in [(seen, "\\Seen"), (flagged, "\\Flagged")] {
                    if let Some(on) = on {
                        let op = if on { "+FLAGS.SILENT" } else { "-FLAGS.SILENT" };
                        let _: Vec<_> = s
                            .uid_store(uid.to_string(), format!("{op} ({name})"))
                            .await?
                            .try_collect()
                            .await?;
                    }
                }
                Ok(Value::Null)
            })
        })
        .await
    }

    /// To another mailbox — Deleted Messages for "delete", Archive for "archive". From
    /// Deleted Messages, delete is for good.
    pub async fn move_to(&mut self, mailbox: String, uid: u32, target: String) -> Result<Value> {
        self.retry(move |m| {
            let (mailbox, target) = (mailbox.clone(), target.clone());
            Box::pin(async move {
                m.select(&mailbox).await?;
                let s = m.session().await?;
                if mailbox == TRASH && target == TRASH {
                    let _: Vec<_> = s
                        .uid_store(uid.to_string(), "+FLAGS.SILENT (\\Deleted)")
                        .await?
                        .try_collect()
                        .await?;
                    let _: Vec<_> = s.uid_expunge(uid.to_string()).await?.try_collect().await?;
                } else {
                    s.uid_mv(uid.to_string(), &target).await?;
                }
                Ok(Value::Null)
            })
        })
        .await
    }

    pub async fn send(&mut self, args: &Value) -> Result<Value> {
        use lettre::message::{header::ContentType, Mailbox};
        use lettre::transport::smtp::authentication::Credentials;
        use lettre::{AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor};

        let email = self
            .email
            .clone()
            .ok_or_else(|| anyhow!("Configure o e-mail do iCloud primeiro."))?;
        let password = Self::password(&email)?;
        let strings = |k: &str| -> Vec<String> {
            args.get(k)
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(|v| v.as_str().map(|s| s.trim().to_string()))
                        .filter(|s| !s.is_empty())
                        .collect()
                })
                .unwrap_or_default()
        };
        let to = strings("to");
        if to.is_empty() {
            return Err(anyhow!("Coloque ao menos um destinatário."));
        }
        let from: Mailbox = match args.get("fromName").and_then(Value::as_str) {
            Some(n) if !n.is_empty() => format!("{n} <{email}>").parse()?,
            _ => email.parse()?,
        };
        let mut b = Message::builder()
            .from(from)
            .subject(args.get("subject").and_then(Value::as_str).unwrap_or(""));
        for t in &to {
            b = b.to(t.parse().map_err(|_| anyhow!("Endereço inválido: {t}"))?);
        }
        for c in strings("cc") {
            b = b.cc(c.parse().map_err(|_| anyhow!("Endereço inválido: {c}"))?);
        }
        if let Some(r) = args.get("inReplyTo").and_then(Value::as_str) {
            let id = format!("<{}>", r.trim_matches(|c| c == '<' || c == '>'));
            b = b.in_reply_to(id.clone()).references(id);
        }
        let message = b.header(ContentType::TEXT_PLAIN).body(
            args.get("text")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
        )?;
        let raw = message.formatted();
        let mailer = AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(SMTP_HOST)?
            .credentials(Credentials::new(email.clone(), password))
            .build();
        mailer
            .send(message)
            .await
            .map_err(|e| anyhow!("O iCloud não aceitou o envio: {e}"))?;
        // A copy in Sent Messages, read, as Mail files one.
        let _ = self
            .retry(move |m| {
                let raw = raw.clone();
                Box::pin(async move {
                    m.session()
                        .await?
                        .append(SENT, Some("(\\Seen)"), None, &raw)
                        .await?;
                    Ok(())
                })
            })
            .await;
        Ok(Value::Null)
    }
}

fn strip_tags(html: &str) -> String {
    let mut out = String::with_capacity(html.len() / 2);
    let mut inside = false;
    let lower = html.to_lowercase();
    // Skip <style> and <script> blocks entirely.
    let mut skip_until: Option<usize> = None;
    for (i, c) in html.char_indices() {
        if let Some(end) = skip_until {
            if i < end {
                continue;
            }
            skip_until = None;
        }
        if c == '<' {
            for tag in ["<style", "<script", "<head"] {
                if lower[i..].starts_with(tag) {
                    let close = format!("</{}", &tag[1..]);
                    if let Some(p) = lower[i..].find(&close) {
                        skip_until = Some(i + p);
                    }
                }
            }
            inside = true;
            continue;
        }
        if c == '>' {
            inside = false;
            out.push(' ');
            continue;
        }
        if !inside {
            out.push(c);
        }
    }
    out.replace("&nbsp;", " ")
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
}

fn preview_of(text: &str) -> String {
    let flat: String = text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('>'))
        .collect::<Vec<_>>()
        .join(" ");
    let collapsed = flat.split_whitespace().collect::<Vec<_>>().join(" ");
    collapsed.chars().take(180).collect()
}
