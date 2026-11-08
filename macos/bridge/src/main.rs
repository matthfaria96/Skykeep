//! skykeep-cloud — the iCloud half of Skykeep for Mac.
//!
//! The Swift app starts this inside its own bundle and speaks JSON, one line per message, over
//! stdin and stdout: `{"id":1,"cmd":"notes.sync","args":{…}}` in, `{"id":1,"ok":true,"result":…}`
//! out. It is the same code the Windows and Linux Skykeep uses — Apple's own sign-in (SRP and
//! two-factor), the session kept in the Mac's keychain, Notes read and written over CloudKit —
//! so a note edited here is edited in iCloud, checklists and formatting intact, on every device.
//! Nothing here opens another app.

mod mail;

use anyhow::{anyhow, Result};
use quince_auth::{Client, SignInOutcome};
use quince_core::notes::{Note, TRASH_FOLDER};
use quince_core::Library;
use serde_json::{json, Value};
use std::path::PathBuf;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

/// What a CloudKit call needs: the session's HTTP client, the CloudKit root and its parameters.
type CloudKit = (reqwest::Client, String, Vec<(String, String)>);

struct Bridge {
    client: Client,
    library: Library,
    resumed: bool,
    mail: mail::Mail,
}

fn state_dir() -> PathBuf {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/tmp"));
    home.join("Library/Application Support/Skykeep/iCloud")
}

impl Bridge {
    fn open() -> Result<Self> {
        let dir = state_dir();
        std::fs::create_dir_all(&dir)?;
        let client = Client::new(&dir).map_err(|e| anyhow!(e.to_string()))?;
        let library = Library::open(&dir.join("library")).map_err(|e| anyhow!(e.to_string()))?;
        Ok(Self {
            client,
            library,
            resumed: false,
            mail: mail::Mail::load(),
        })
    }

    /// Where the session stands: "signedIn", "code" (Apple wants the six digits) or "signedOut".
    async fn status(&mut self) -> Result<Value> {
        if !self.resumed {
            self.resumed = true;
            match self.client.resume().await {
                Ok(true) => return Ok(json!({ "state": "signedIn" })),
                Ok(false) => {}
                Err(e) if self.client.has_stored_session() => {
                    // Offline or Apple busy with a session kept: still signed in, read what is here.
                    return Ok(
                        json!({ "state": "signedIn", "offline": true, "note": e.to_string() }),
                    );
                }
                Err(_) => {}
            }
        } else if self.client.session().service_url("ckdatabasews").is_some() {
            return Ok(json!({ "state": "signedIn" }));
        }
        Ok(json!({ "state": if self.client.awaiting_code() { "code" } else { "signedOut" } }))
    }

    fn context(&self) -> Result<CloudKit> {
        let root = self
            .client
            .session()
            .service_url("ckdatabasews")
            .ok_or_else(|| anyhow!("Entre com sua conta Apple para usar as Notas do iCloud."))?;
        Ok((
            self.client.http().clone(),
            root,
            self.client.service_params(),
        ))
    }

    /// A lapsed session is taken up again once before giving up.
    async fn revive(&mut self) {
        let _ = self.client.resume().await;
    }

    async fn sync_notes(&mut self) -> Result<()> {
        for attempt in 0..2 {
            let (http, root, params) = self.context()?;
            let since = self
                .library
                .sync_token("notes")
                .map_err(|e| anyhow!(e.to_string()))?;
            match quince_icloud::notes::fetch_cloudkit(&http, &root, params, since.as_deref()).await
            {
                Ok(fetched) => {
                    quince_icloud::notes::apply_cloudkit(&mut self.library, fetched)
                        .map_err(|e| anyhow!(e.to_string()))?;
                    return Ok(());
                }
                Err(e) if attempt == 0 => {
                    let _ = e;
                    self.revive().await;
                }
                Err(e) => return Err(anyhow!(e.to_string())),
            }
        }
        Ok(())
    }

    fn notes_list(&self) -> Result<Value> {
        let folders = self
            .library
            .note_folders()
            .map_err(|e| anyhow!(e.to_string()))?;
        let mut notes: Vec<Note> = self
            .library
            .notes_in(None)
            .map_err(|e| anyhow!(e.to_string()))?;
        if let Ok(trash) = self.library.notes_in(Some(TRASH_FOLDER)) {
            for n in trash {
                if !notes.iter().any(|m| m.guid == n.guid) {
                    notes.push(n);
                }
            }
        }
        Ok(json!({ "folders": folders, "notes": notes, "trash": TRASH_FOLDER }))
    }

    fn note(&self, guid: &str) -> Result<Note> {
        self.library
            .note(guid)
            .map_err(|e| anyhow!(e.to_string()))?
            .ok_or_else(|| anyhow!("Essa nota não está mais no iCloud."))
    }

    async fn handle(&mut self, cmd: &str, args: &Value) -> Result<Value> {
        let s = |k: &str| args.get(k).and_then(Value::as_str).map(ToString::to_string);
        match cmd {
            "status" => self.status().await,
            "signIn" => {
                let id = s("appleId").ok_or_else(|| anyhow!("Falta a conta Apple."))?;
                let pw = s("password").ok_or_else(|| anyhow!("Falta a senha."))?;
                match self
                    .client
                    .sign_in(&id, &pw)
                    .await
                    .map_err(|e| anyhow!(e.to_string()))?
                {
                    SignInOutcome::Complete => Ok(json!({ "state": "signedIn" })),
                    SignInOutcome::NeedsTwoFactor => Ok(json!({ "state": "code" })),
                }
            }
            "submitCode" => {
                let code = s("code").ok_or_else(|| anyhow!("Falta o código."))?;
                self.client
                    .submit_two_factor(&code)
                    .await
                    .map_err(|e| anyhow!(e.to_string()))?;
                Ok(json!({ "state": "signedIn" }))
            }
            "resendCode" => {
                self.client
                    .resend_to_devices()
                    .await
                    .map_err(|e| anyhow!(e.to_string()))?;
                Ok(Value::Null)
            }
            "signOut" => {
                self.client.sign_out().map_err(|e| anyhow!(e.to_string()))?;
                Ok(json!({ "state": "signedOut" }))
            }
            "notes.list" => self.notes_list(),
            "notes.sync" => {
                self.sync_notes().await?;
                self.notes_list()
            }
            "note.body" => {
                let guid = s("guid").ok_or_else(|| anyhow!("Falta a nota."))?;
                let body = self
                    .library
                    .note_body(&guid)
                    .map_err(|e| anyhow!(e.to_string()))?;
                let runs: Value = self
                    .library
                    .note_runs(&guid)
                    .map_err(|e| anyhow!(e.to_string()))?
                    .and_then(|r| serde_json::from_str(&r).ok())
                    .unwrap_or(Value::Array(vec![]));
                Ok(json!({ "body": body, "runs": runs }))
            }
            "note.check" => {
                let guid = s("guid").ok_or_else(|| anyhow!("Falta a nota."))?;
                let start = args
                    .get("start")
                    .and_then(Value::as_u64)
                    .ok_or_else(|| anyhow!("Falta a linha."))?;
                let change = if let Some(style) = args.get("style").and_then(Value::as_u64) {
                    quince_icloud::ckdocument::ParagraphChange::Style(style)
                } else {
                    quince_icloud::ckdocument::ParagraphChange::Checked(
                        args.get("checked")
                            .and_then(Value::as_bool)
                            .unwrap_or(false),
                    )
                };
                let expect = s("expect");
                let note = self.note(&guid)?;
                let (http, root, params) = self.context()?;
                quince_icloud::notes::set_paragraph_cloudkit(
                    &http,
                    &root,
                    params,
                    &note,
                    start,
                    change,
                    expect.as_deref(),
                )
                .await
                .map_err(|e| anyhow!(e.to_string()))?;
                self.sync_notes().await?;
                Ok(Value::Null)
            }
            "note.save" => {
                let guid = s("guid").ok_or_else(|| anyhow!("Falta a nota."))?;
                let body = s("body").ok_or_else(|| anyhow!("Falta o texto."))?;
                let base = s("base");
                let note = self.note(&guid)?;
                if !note.editable {
                    return Err(anyhow!("Esta nota tem uma formatação que não dá para regravar com segurança fora do Notas, então fica só para leitura."));
                }
                let (http, root, params) = self.context()?;
                let saved = quince_icloud::notes::edit_cloudkit_from(
                    &http,
                    &root,
                    params,
                    &note,
                    base.as_deref(),
                    &body,
                )
                .await
                .map_err(|e| anyhow!(e.to_string()))?;
                self.sync_notes().await?;
                Ok(json!({ "body": saved }))
            }
            "note.create" => {
                let body = s("body").ok_or_else(|| anyhow!("Falta o texto."))?;
                let folder = s("folder");
                let (http, root, params) = self.context()?;
                let guid = quince_icloud::notes::create_cloudkit(
                    &http,
                    &root,
                    params,
                    folder.as_deref(),
                    &body,
                    None,
                )
                .await
                .map_err(|e| anyhow!(e.to_string()))?;
                self.sync_notes().await?;
                Ok(json!({ "guid": guid }))
            }
            "note.delete" => {
                let guid = s("guid").ok_or_else(|| anyhow!("Falta a nota."))?;
                let note = self.note(&guid)?;
                let (http, root, params) = self.context()?;
                quince_icloud::notes::delete_cloudkit(&http, &root, params, &note)
                    .await
                    .map_err(|e| anyhow!(e.to_string()))?;
                self.sync_notes().await?;
                Ok(Value::Null)
            }
            "note.move" => {
                let guid = s("guid").ok_or_else(|| anyhow!("Falta a nota."))?;
                let folder = s("folder").ok_or_else(|| anyhow!("Falta a pasta."))?;
                let note = self.note(&guid)?;
                let (http, root, params) = self.context()?;
                quince_icloud::notes::move_cloudkit(
                    &http,
                    &root,
                    params,
                    &note,
                    Some(folder.as_str()),
                )
                .await
                .map_err(|e| anyhow!(e.to_string()))?;
                self.sync_notes().await?;
                Ok(Value::Null)
            }
            "mail.status" => Ok(self.mail.status()),
            "mail.setup" => {
                let email = s("email").ok_or_else(|| anyhow!("Falta o e-mail."))?;
                let password = s("password").ok_or_else(|| anyhow!("Falta a senha de app."))?;
                self.mail.setup(&email, &password).await
            }
            "mail.signOut" => self.mail.sign_out().await,
            "mail.list" => {
                let mailbox = s("mailbox").unwrap_or_else(|| "INBOX".into());
                let limit = args.get("limit").and_then(Value::as_u64).unwrap_or(150) as u32;
                self.mail.list(mailbox, limit).await
            }
            "mail.body" => {
                let mailbox = s("mailbox").unwrap_or_else(|| "INBOX".into());
                let uid = args
                    .get("uid")
                    .and_then(Value::as_u64)
                    .ok_or_else(|| anyhow!("Falta a mensagem."))? as u32;
                self.mail.body(mailbox, uid).await
            }
            "mail.flag" => {
                let mailbox = s("mailbox").unwrap_or_else(|| "INBOX".into());
                let uid = args
                    .get("uid")
                    .and_then(Value::as_u64)
                    .ok_or_else(|| anyhow!("Falta a mensagem."))? as u32;
                let seen = args.get("seen").and_then(Value::as_bool);
                let flagged = args.get("flagged").and_then(Value::as_bool);
                self.mail.flag(mailbox, uid, seen, flagged).await
            }
            "mail.move" => {
                let mailbox = s("mailbox").unwrap_or_else(|| "INBOX".into());
                let uid = args
                    .get("uid")
                    .and_then(Value::as_u64)
                    .ok_or_else(|| anyhow!("Falta a mensagem."))? as u32;
                let target = s("target").unwrap_or_else(|| mail::TRASH.into());
                self.mail.move_to(mailbox, uid, target).await
            }
            "mail.send" => self.mail.send(args).await,
            other => Err(anyhow!("Comando desconhecido: {other}")),
        }
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    let mut out = tokio::io::stdout();
    let mut bridge = match Bridge::open() {
        Ok(b) => b,
        Err(e) => {
            let line = json!({ "id": 0, "ok": false, "error": e.to_string() }).to_string() + "\n";
            out.write_all(line.as_bytes()).await?;
            return Ok(());
        }
    };
    let mut lines = BufReader::new(tokio::io::stdin()).lines();
    while let Some(line) = lines.next_line().await? {
        let Ok(msg) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let id = msg.get("id").cloned().unwrap_or(Value::Null);
        let cmd = msg.get("cmd").and_then(Value::as_str).unwrap_or("");
        let args = msg.get("args").cloned().unwrap_or(Value::Null);
        let reply = match bridge.handle(cmd, &args).await {
            Ok(result) => json!({ "id": id, "ok": true, "result": result }),
            Err(e) => json!({ "id": id, "ok": false, "error": e.to_string() }),
        };
        out.write_all((reply.to_string() + "\n").as_bytes()).await?;
        out.flush().await?;
    }
    Ok(())
}
