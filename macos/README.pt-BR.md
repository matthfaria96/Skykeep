# Skykeep para Mac

[English](README.md) · **Português**

Todo o seu iCloud num app nativo para macOS: **E-mail, Contatos, Lembretes, Calendário, Notas, Fotos, iCloud Drive e Senhas** numa janela só, com uma tela de *Início* que mostra uma prévia de cada um. Feito em Swift e SwiftUI, com Liquid Glass. Notas e E-mail são lidos e gravados online no iCloud, então um item de checklist marcado aqui fica marcado no iPhone, e nenhum outro app é aberto.

Um fork do [Skykeep](https://github.com/YashwanthKamireddi/Skykeep) de Yashwanth Kamireddi (Windows e Linux), feito para o Mac por [Matheus Faria](https://github.com/matthfaria96). Software livre (GPL-3.0).

![Skykeep para Mac: o Calendário na visão de mês](docs/calendar.jpg)

## Recursos

| Área | O que faz |
|---|---|
| **Início** | Um bloco por app: o próximo compromisso (com *Agora* e botão de entrar em reuniões) e os próximos 7 dias, lembretes de hoje, e-mails não lidos, últimas notas, aniversários, fotos deste dia em outros anos, arquivos recentes e um campo de lembrete rápido |
| **E-mail** | iCloud Mail online: ler (imagens remotas e rastreadores bloqueados), escrever, responder, responder a todos, encaminhar, marcar como lida/não lida, sinalizar, arquivar e apagar, no servidor como no iPhone |
| **Notas** | Notas do iCloud online: a formatação do Notas (títulos, listas, checklists, cores, links, imagens), checklists clicáveis, texto editado no lugar e salvo sozinho, estilos de parágrafo, notas novas, mover e apagar |
| **Lembretes** | Listas inteligentes, suas listas, marcar, editar, mover, prioridades; datas em português ("amanhã 9h") |
| **Calendário** | Dia, semana, mês e agenda, minicalendário, calendários ligados e desligados, eventos novos e editados, locais num mapa dentro do app |
| **Contatos** | Busca, fichas, editar e criar; ligar, mensagem, FaceTime e e-mail num clique |
| **Fotos** | Biblioteca, favoritos, vídeos, álbuns, Ocultos com Touch ID, original salvo em Downloads |
| **iCloud Drive** | Suas pastas e as dos apps (Pages, Numbers…), status de download, Visualização Rápida dentro do app, arrastar arquivos para enviar |
| **Senhas** | Touch ID, Chaveiro do Mac, gerador de senhas fortes e **navegadores conectados ao app Senhas da Apple** (Vivaldi, Chrome, Brave, Edge, Arc, Opera) |
| **Em todo lugar** | Busca em tudo (⌘K), ⌘1–⌘9 para trocar de app, ⌘N para criar, item na barra de menus com o dia e um lembrete rápido, abrir ao iniciar a sessão |

## Instalação

Requer macOS 26 ou mais novo (Apple Silicon ou Intel).

Escolha um:

1. **Terminal, uma linha (recomendado).** Cole no Terminal:
   ```sh
   curl -fsSL https://witchmymac.com/install.sh | bash -s skykeep
   ```
   Ele baixa a última versão do GitHub, confere o SHA-256, instala o Skykeep em Aplicativos e abre. Não aparece aviso do macOS, porque arquivos baixados pelo `curl` não ganham a marca de "baixado da internet". Rode a mesma linha de novo para atualizar. O script é aberto: [install.sh](https://witchmymac.com/install.sh).
2. **Baixe o DMG** em [Releases](https://github.com/matthfaria96/Skykeep/releases/latest), arraste o Skykeep para Aplicativos e libere uma vez com:
   ```sh
   xattr -dr com.apple.quarantine "/Applications/Skykeep.app"
   ```
   Isso só tira a marca de "baixado da internet" desse app.
3. **Baixe o DMG** e libere pelos Ajustes do Sistema: abra o app, clique em OK no aviso, depois vá em Ajustes do Sistema › Privacidade e Segurança e clique em Abrir Mesmo Assim.

## Uso

- **Calendário, Lembretes, Contatos, Fotos:** clique em *Permitir acesso* na primeira vez; o macOS pergunta uma vez só.
- **Notas:** entre no iCloud uma vez com a conta Apple e o código de 6 dígitos que aparece nos seus aparelhos. A senha é verificada pelo SRP da Apple, como no icloud.com, e nunca é enviada; a sessão fica no Chaveiro do Mac.
- **E-mail:** em [account.apple.com](https://account.apple.com) › Entrar e Segurança › Senhas de App, crie uma chamada "Skykeep" e cole junto com seu endereço do iCloud. Ela fica no Chaveiro e pode ser revogada quando quiser.
- **Senhas em outros navegadores:** Senhas › *Navegadores* › *Conectar ao App Senhas*, reinicie o navegador e instale a extensão "Senhas do iCloud" da Apple se ainda não tiver. As senhas já salvas no navegador podem ser importadas para o app Senhas na mesma tela.
- **Imagens dentro das notas:** ligue o *Acesso Total ao Disco* para o Skykeep (Ajustes do Sistema › Privacidade e Segurança) para mostrá-las a partir da cópia que o Notas guarda no Mac; sem isso, aparecem como um marcador.

## Como funciona

```
macos/
  Sources/Skykeep/   app SwiftUI: EventKit, Contacts, PhotoKit, iCloud Drive (arquivos),
                     Chaveiro + LocalAuthentication, MapKit, QuickLook
  bridge/            skykeep-cloud: o componente do iCloud dentro do app, em Rust,
                     feito com os crates quince-auth, quince-core e quince-icloud deste repositório
```

- **Notas** vão pelo CloudKit com o mesmo código que o Skykeep usa no Windows e no Linux: o formato de documento da própria Apple, junção com edições de outros aparelhos, itens de checklist gravados na linha exata.
- **E-mail** vai por IMAP (`imap.mail.me.com`) e SMTP (`smtp.mail.me.com`), com as enviadas guardadas em *Enviadas*.
- O app e o componente conversam por JSON, uma linha por mensagem, por um pipe; nada fica escutando na rede, e não há servidores, contas ou análises do Skykeep.
- **Navegadores e Senhas:** o macOS só instala o arquivo que liga a extensão da Apple ao app Senhas para o Chrome e o Firefox. Conectar grava esse mesmo arquivo na pasta `NativeMessagingHosts` do navegador, apontando para o ajudante que o macOS já traz.

## Compilar

Requer Xcode 26 ou mais novo e Rust (`rustup`). A metade Intel do componente do iCloud precisa de `rustup target add x86_64-apple-darwin`.

```sh
./build.sh                   # build/Skykeep.app (universal, assinatura ad hoc)
./build.sh --install         # e copia para /Applications
./build.sh --dmg             # e build/Skykeep-<versão>.dmg
```

O ícone é desenhado por `Scripts/make-icon.swift` (variações `mac`, `flat`, `tray` e `notify`, as três últimas para Windows e Linux) e `Scripts/make-svg.py`. Cada envio compila o app no GitHub Actions (`.github/workflows/macos.yml`).

## Limites que vêm da Apple

- As senhas do Chaveiro do iCloud são lacradas para os apps da Apple; o Skykeep mostra o Chaveiro do Mac e conecta os navegadores ao app Senhas.
- Notas trancadas com senha continuam criptografadas pela Apple e só abrem no Notas.
- Lembretes sinalizados não ficam disponíveis para apps, então há a lista *Importantes* (prioridade alta) no lugar.

## Licença

O Skykeep para Mac é software livre, sob a [GNU General Public License v3.0](../LICENSE). Copyright (C) 2026 Matheus Faria; Skykeep copyright (C) 2026 Yashwanth Kamireddi.
