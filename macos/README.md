# Skykeep para Mac

Feito por **Matheus Faria** ([@matthfaria96](https://github.com/matthfaria96)), num fork do
[Skykeep](https://github.com/YashwanthKamireddi/Skykeep) de Yashwanth Kamireddi. GPL-3.0.

O Skykeep de Windows e Linux reimplementa o iCloud para quem não tem Mac. No Mac o iCloud já
está no sistema, então esta versão é **nativa em SwiftUI** e lê direto o que o macOS já mantém
sincronizado. Sem login, sem servidor, sem cópia dos seus dados — e **sem abrir nenhum outro
app**: nada de Notas, Mail, Mapas ou Finder abrindo por conta própria.

| Seção | Como funciona no Mac |
|---|---|
| **Hoje** | Widgets: agenda em linha do tempo (com “Agora” e “Entrar” em reuniões), lembretes do dia, e-mails não lidos, aniversários, notas, “Neste dia” (Fotos), arquivos recentes e captura rápida |
| **E-mail** | iCloud Mail online (IMAP/SMTP, com senha de app): ler, responder, encaminhar, enviar, marcar como lida, sinalizar, arquivar e apagar — no servidor, igual no iPhone; conteúdo remoto e rastreadores bloqueados |
| **Lembretes** | EventKit: listas inteligentes, marcar, editar, mover, datas em português (“amanhã 9h”) |
| **Calendário** | EventKit: dia, semana, mês e agenda, criar/editar eventos, local num mapa dentro do app |
| **Notas** | Direto no iCloud (CloudKit), como no Skykeep para Windows/Linux: editar no lugar, marcar checklists, títulos e listas, criar, mover e apagar — em todos os dispositivos |
| **Contatos** | Contacts: buscar, editar, criar, ligar, mensagem, FaceTime, e-mail |
| **Fotos** | PhotoKit: biblioteca, álbuns, favoritos, vídeos, Ocultos com Touch ID, exportar original |
| **iCloud Drive** | Pastas do Drive e dos apps (Pages, Numbers…), status de download, pré-visualização dentro do app, arrastar para enviar |
| **Senhas** | Touch ID, Chaveiro do Mac, gerador de senhas e **conexão dos navegadores ao app Senhas** (veja abaixo) |

E ainda: busca em tudo (⌘K), ⌘1–⌘8 para trocar de seção, ⌘N para criar, um item na barra de
menus com o dia e um lembrete rápido, e a opção de abrir ao iniciar a sessão.

## Senhas iguais no Safari, Vivaldi, Chrome, Brave, Edge, Arc e Opera

A Apple tem a extensão “Senhas do iCloud” para navegadores baseados no Chrome, mas o macOS só
instala o arquivo que liga a extensão ao app Senhas (`com.apple.passwordmanager.json`) para o
Chrome e o Firefox. Nos outros a extensão instala e nunca conecta. Em **Senhas › Navegadores**
o Skykeep:

1. **Conecta** o navegador: grava esse mesmo arquivo na pasta `NativeMessagingHosts` dele,
   apontando para o ajudante que o próprio macOS traz. Depois de reiniciar o navegador, a
   extensão preenche e salva direto no app Senhas — nos dois sentidos, sempre, sem cópias.
2. Abre a página da **extensão** na Chrome Web Store, se ainda não estiver instalada.
3. **Importa** as senhas que já estão salvas no navegador: lê o banco do navegador em modo
   somente leitura (a chave vem do Chaveiro, com o “Permitir” do Mac), grava um CSV no formato
   do app Senhas em Downloads (só você lê), abre o app Senhas para *Arquivo › Importar Senhas…*
   e apaga o arquivo (sobrescrevendo antes) quando você confirma, ou em 15 minutos.

## Compilar

Requer macOS 26 ou superior e o Xcode instalado.

```sh
./build.sh            # gera build/Skykeep.app (precisa do Xcode e do Rust)
./build.sh --install  # copia para /Applications
./build.sh --run      # compila e abre
```

A assinatura é ad-hoc, o que basta para usar neste Mac. O requisito da assinatura fixa o
identificador do app, então as permissões dadas uma vez continuam valendo depois de recompilar. Para distribuir, assine com um
Developer ID (`codesign --options runtime --sign "Developer ID Application: …"`) e faça a
notarização com `xcrun notarytool`.

## Notas pelo iCloud

As Notas usam o mesmo código do Skykeep para Windows e Linux (`crates/quince-auth` e
`crates/quince-icloud`), compilado como `skykeep-cloud` (em `macos/bridge`) e embutido no app.
Você entra uma vez com a conta Apple e o código de dois fatores; a senha é verificada com o SRP da
Apple (nunca é enviada) e a sessão fica no Chaveiro do Mac. O app Notas nunca é aberto.

## E-mail pelo iCloud

O mesmo componente (`skykeep-cloud`) fala com o iCloud Mail como qualquer app de e-mail:
IMAP em `imap.mail.me.com` e SMTP em `smtp.mail.me.com`. A Apple pede uma **senha de app**,
criada uma vez em account.apple.com › Entrar e Segurança › Senhas de App; ela fica no Chaveiro.

## Acesso Total ao Disco (imagens das notas)

O Notas e o Mail não têm API pública, e a única forma de lê-los sem abrir esses apps é ler os
arquivos deles no disco, que o macOS protege. Em **Ajustes do Sistema › Privacidade e Segurança ›
Acesso Total ao Disco**, ligue o Skykeep. O Skykeep só **lê** esses arquivos (abre o SQLite em
modo somente leitura) e nunca escreve neles.

## Limites que vêm da Apple

- **Senhas do iCloud (Chaveiro do iCloud)** ficam lacradas para os apps da Apple. O Skykeep mostra
  as do Chaveiro do Mac e conecta os navegadores ao app Senhas (veja acima).
- **Notas trancadas com senha** continuam criptografadas pela Apple e só abrem no app Notas.
- **Lembretes sinalizados** não aparecem no EventKit, por isso o app tem a lista “Importantes”
  (prioridade alta) no lugar.
- **O que abre outro app, só quando você clica:** ligar ou mandar mensagem para um contato,
  entrar numa reunião por vídeo, um link num e-mail, “Abrir com o App Padrão” no Drive e os
  Ajustes do Sistema para dar permissões.
