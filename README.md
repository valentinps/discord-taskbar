# discord-taskbar

Your Discord voice status, live in the Windows taskbar — channel, server, who's
in the call, who's talking, and your own mute/deafen state.

Built in the spirit of [TrafficMonitor](https://github.com/zhongyang219/TrafficMonitor):
a real Win32 child window of `Shell_TrayWnd`, so it works with third-party
taskbars like StartAllBack instead of fighting them.

```
┌──────────────────────────────────────────────────────────┐
│  Conclave  ·  My Server        (◕)(◕)(◕)      🎙  🎧      │
└──────────────────────────────────────────────────────────┘
```

## What it does

- Shows the server's icon and the voice channel you're in. The server name can
  be shown instead, in which case the channel comes first so the server is what
  gets ellipsised when space is tight.
- Circular avatars for everyone in the call, with Discord's green ring around
  whoever is speaking. Overlapping or spaced out, and optionally in a fixed
  order so faces don't shuffle mid-conversation.
- Mute and deafen badges on each avatar, and your own state as separate
  microphone/headphone glyphs — **click them to toggle mute and deafen**.
- A hang-up button that leaves the voice channel.
- Shows on whichever displays you choose, one widget per taskbar.
- Per-participant control: scroll over someone to set their volume, middle-click
  for a configurable action, click for a menu.
- Hides completely when you're not in a call — no pixels, no repaints.
- Rebuilds itself automatically when `explorer.exe` restarts.
- Notification-area icon for focusing Discord, reconnecting, re-authorizing,
  restarting (which reloads the config) and quitting.

## Interaction

| Where | Input | What happens |
|---|---|---|
| Server icon, channel name | Click | Focus Discord |
| A participant | Click | Their menu — local mute, drag-or-click volume bar |
| The menu's volume bar | Drag | Adjusts live, and the menu stays open |

Clicking the same participant again closes their panel; clicking a different
one moves the panel to them. Opening a panel dismisses the small readout — the same number in
two places at once is just noise. Inside the panel, controls that change the
volume (the bar, **Reset volume**) leave it open; commands close it.
| A participant | Scroll | Their volume, with a readout above the taskbar |
| A participant | Middle-click | `middle_click` from the config |
| Your mic / headphones | Click | Toggle mute / deafen |
| Hang-up button | Click | Leave the voice channel |
| Anywhere | Right-click | The tray menu |

The cursor becomes a hand over anything clickable.

The volume readout appears in its own small panel above the taskbar, centred on
the screen, rather than inside the widget. It stays up for as long as the
pointer remains on that person and vanishes the moment it moves off them —
tying its lifetime to the pointer rather than a timer is what makes it feel
attached to what you are doing. That is deliberate: drawing it in place would cover the
avatar being adjusted, and once that avatar is gone the next scroll notch has
nothing to hit — you could only ever change the volume by one step. Keeping the
widget untouched is what makes scrolling repeatedly work.

**What is not here, and why.** Server mute, server deafen and disconnecting
*another* user are absent. They are real permissions ordinary members can hold,
so this is worth being precise about:

- The endpoint is `PATCH /guilds/{guild_id}/members/{user_id}` with `mute`,
  `deaf`, or `channel_id: null` to disconnect, gated on `MUTE_MEMBERS`
  (`1 << 22`), `DEAFEN_MEMBERS` (`1 << 23`) and `MOVE_MEMBERS` (`1 << 24`).
- **No OAuth2 scope grants it.** The scope list has `guilds`, `guilds.join` and
  `guilds.members.read` — there is no `guilds.members.write` or equivalent. The
  endpoint accepts bot tokens and user tokens only. This is not a gap that a
  different scope request or partner approval closes.
- RPC has no command for it either, and no way to read whether you hold the
  permission: `GET_CHANNEL` and `GET_GUILD` return no `permissions` field.

The two ways to actually do it are set out in **Server moderation** below.

## Cost

Measured on the release build:

| | idle, before any image fetch | settled |
|---|---|---|
| CPU | 0.03% of one core | 0.03% of one core |
| Private bytes | 2.18 MB | 2.45 MB |
| Working set | 13.1 MB | 15.5 MB |
| Threads | 7 | 9 |
| Handles | 176 | 202 |

Binary is 342 KB and the only dependencies are `windows`, `serde` and
`serde_json`.

The step between the two columns is a one-time cost: the first avatar fetch
brings up WIC, WinHTTP and a COM apartment, which is worth a couple of threads
and a few hundred KB. It does not keep growing — `stress_probe` renders 5,000
frames and private bytes, handle count and GDI object count all stay flat, at
0.34 ms per frame.

Everything is event-driven: the app blocks on Discord's named pipe and repaints
only when something actually changes. The only polling is a 1-second timer that
re-checks the taskbar's position, which is what TrafficMonitor does too.

## Install

For anyone who just wants to run it, `make-installer.sh` produces a single
self-contained `dist/DiscordTaskbarSetup.exe`. It embeds the application, walks
through creating the Discord application below, and installs per-user under
`%LOCALAPPDATA%\Programs\Discord Taskbar` — so it never asks for administrator
rights.

```sh
./make-installer.sh
```

Setup offers to start the widget at sign-in, add Start menu and desktop
shortcuts, and registers an entry in **Settings › Apps**, so it uninstalls the
way any other program does. Removing it leaves your `config.json` alone unless
you tick the box that says otherwise, so reinstalling picks up where you left
off.

The uninstaller is a copy of setup itself: installing writes `uninstall.exe`
alongside the application, and running it stages a copy into `%TEMP%` first,
because a program cannot delete the folder it is running from.

Both binaries link the MSVC C runtime statically, so they import nothing that
does not ship with Windows. A default Rust MSVC build pulls in
`VCRUNTIME140.dll` from the Visual C++ Redistributable, and on a machine
without it the process dies at load time, before `main` runs — the app just
never appears, with nothing to go on. That is a bad failure to hand somebody
along with an installer, and the static CRT costs about 100 KB.

No installer toolchain is involved — no Inno Setup, no NSIS, no WiX. It is a
second Rust binary in `installer/`, which embeds the first with `include_bytes!`.
Cargo cannot express "build A, then embed A into B", which is why the two-step
build lives in a script.

## Setup from source

### 1. Create a Discord application

The app talks to Discord's official local RPC API, which needs an application ID
of your own. For apps Discord hasn't approved, the `rpc` scope is restricted to
the application's **owner** — which is you, so this works without any approval
process.

1. Go to <https://discord.com/developers/applications> → **New Application**.
   The name is only ever shown to you.
2. Open the **OAuth2** tab:
   - Copy the **Client ID**.
   - **Reset Secret** and copy the **Client Secret**.
   - Under **Redirects**, add `http://localhost` and **Save Changes**.
     (Nothing ever visits this URL — the authorization code comes back over the
     pipe — but it has to be registered.)

### 2. Configure

The installer asks for both values on its second page and writes them for you.
Otherwise, open **Settings** from the widget's tray icon and paste them there,
or edit `%APPDATA%\discord-taskbar\config.json` by hand:

```json
{
  "discord": {
    "client_id": "your-client-id",
    "client_secret": "your-client-secret"
  }
}
```

This file is outside the repository and is gitignored. So is `token.json`, which
caches your OAuth token so the consent dialog only ever appears once.

### 3. Run

```sh
cargo build --release
./target/release/discord-taskbar.exe
```

The first run pops an authorization dialog in Discord asking for `rpc`,
`rpc.voice.read` and `rpc.voice.write`. Accept it once.

`rpc.voice.write` is what makes the microphone and headphone glyphs clickable.
Two things are worth knowing about it: Discord lets only one RPC client modify
voice settings at a time, so another connected app that writes them first locks
everyone else out until it disconnects; and if the app is ever updated to need a
scope the cached token doesn't have, it re-prompts rather than silently losing
the capability.

To start it with Windows, put a shortcut to the exe in `shell:startup` — or let
the installer add the registry entry for you.

## Configuration

### The settings window

Right-click the notification-area icon and choose **Settings**, or run
`discord-taskbar.exe --settings`. Every option in the table below appears there,
grouped into sections across two columns, with a button that opens the Discord
developer portal beside the credential fields.

The window is driven by a single declarative table in `src/ui/settings.rs`:

```rust
struct Field {
    section: &'static str,
    label: &'static str,
    kind: Kind,                    // Toggle | Number | Text | Choice
    get: fn(&Config) -> String,
    set: fn(&mut Config, &str),
}
```

Adding a setting means adding one row — creating the control, filling it in,
reading it back and laying it out all follow from `kind`. Saving re-reads the
file from disk first, so keys the window does not show are preserved rather than
overwritten with defaults, and changes apply immediately: fonts, icon fonts and
the avatar cache are rebuilt and every surface repainted without a restart.

### The file

Everything below lives under `"appearance"` in `config.json`. Metrics are in
96-DPI pixels and are scaled automatically; colours are `#RRGGBB` or
`#RRGGBBAA`.

| Key | Default | Meaning |
|---|---|---|
| `background` | `#2B2D31D8` | Widget background |
| `text` | `#DBDEE1` | Channel name |
| `text_dim` | `#949BA4` | Server name, inactive icons |
| `speaking` | `#23A559` | Speaking ring |
| `danger` | `#DA373C` | Mute/deafen |
| `corner_radius` | `6` | Background corner rounding |
| `padding` | `10` | Inner horizontal padding |
| `spacing` | `8` | Gap between elements |
| `avatar_size` | `22` | Avatar diameter |
| `avatar_overlap` | `6` | How much adjacent avatars overlap. `0` butts them together; **negative values leave a gap** |
| `max_avatars` | `6` | Cap on avatars shown |
| `sort_by_speaking` | `true` | Move whoever is talking to the front. Set `false` to keep a fixed order |
| `font_size` | `12` | Label size |
| `icon_size` | `16` | Mic/headphone glyph size |
| `height` | `32` | Widget height. **`0` fills the taskbar** |
| `max_label_width` | `220` | Label width before ellipsising |
| `show_guild_icon` | `true` | Show the server's icon before the channel |
| `guild_icon_size` | `18` | Server icon diameter |
| `show_guild_name` | `false` | Show the server's *name* after the channel |
| `show_self_icons` | `true` | Show your own mic/headphone state |
| `clickable_self_icons` | `true` | Let clicking those glyphs toggle mute/deafen |
| `show_divider` | `true` | Thin rule between participants and your controls |
| `divider` | `#4E5058` | That rule's colour |
| `show_leave_button` | `true` | Hang-up button to leave voice |
| `middle_click` | `local_mute` | Middle-click on a participant: `local_mute`, `volume_reset` or `none` |
| `scroll_volume_step` | `10` | Volume change per wheel notch |
| `monitors` | `["primary"]` | Which displays to show on: `["primary"]`, `["all"]`, or indices like `["0","1"]`. The tray menu's **Show on** submenu edits this |
| `x_offset` / `y_offset` | `0` | Nudge the widget |

With `sort_by_speaking` on, and more people than `max_avatars`, speakers are
kept first, then you, then everyone else alphabetically — so the cut falls on
people who aren't talking. With it off the order is fixed and alphabetical.

### Making it look built in

To drop the panel look and have the status sit directly on the taskbar, make
the background fully transparent and let the widget use the bar's full height:

```json
"background": "#00000000",
"height": 0,
"avatar_size": 28,
"icon_size": 20
```

`height: 0` means "as tall as the taskbar", which is what buys room for larger,
more legible icons. A transparent background leaves nothing but the content,
since the widget already paints the taskbar's own colour underneath.

Set `avatar_overlap` to a negative number to space the avatars apart instead of
stacking them.

## How it works

```
Discord.exe ──(named pipe, event-driven)──► provider thread
                                                 │  VoiceStatus snapshot
                                                 ▼  PostMessage
                                            host window (hidden, top-level)
                                                 │
                                                 ▼
                                            widget window (child of Shell_TrayWnd)
```

Two windows, deliberately:

- The **host** is a hidden top-level window owning all state, the tray icon and
  the timer. It has to be top-level rather than message-only because
  `TaskbarCreated` is a broadcast, and broadcasts only reach top-level windows.
  That message is how the app recovers from an explorer restart.
- The **widget** is a `WS_CHILD` of `Shell_TrayWnd`. Explorer destroys it on
  restart, and the host simply builds another.

### Three things worth knowing

These cost real debugging time and shaped the implementation:

**Cross-process `WS_CHILD` creation fails.** Passing the taskbar's `HWND` to
`CreateWindowExW` with `WS_CHILD` does not work. You have to create a top-level
popup and then call `SetParent`, followed by setting `WS_CHILD` yourself —
`SetParent` does not change the style bits. This is exactly what TrafficMonitor
does, and now the reason is documented. See `examples/parent_probe.rs`.

**`UpdateLayeredWindow` silently does nothing on a child window.** It returns
success, sets no error, and composites nothing. This is why TrafficMonitor
carries a DirectComposition path for its layered rendering mode. Rather than
pull in D3D11 and DirectComposition for a 340×32 pill, this app samples the
taskbar's actual colour off the screen and paints opaquely over it. Sampling
rather than hardcoding means StartAllBack themes and accent colours are followed
automatically. The per-pixel alpha canvas is unchanged underneath, so
anti-aliased circles and rings still work.

**Icon glyphs are worth taking from the OS.** These started as hand-drawn
vectors and looked it — at the 9–20 px they are actually used, proportional
geometry turns to mush and a stroke scaled to 11% of an 11 px glyph is one
pixel. Segoe Fluent Icons ships with Windows 11 and has properly hinted
versions: `E720` microphone, `F781` microphone-off, `E7F6` headphones. There is
no headphones-off glyph in either icon font, so that one gets a hand-drawn
slash over the real glyph. `examples/glyph_probe.rs` is how those code points
were found.

**Finding Discord's window needs more than the class name.**
`Chrome_WidgetWin_1` is the class *every* Chromium app uses, so matching on it
alone is as likely to focus a browser as Discord. The window has to be matched
to a process actually named `Discord.exe` (or Canary/PTB/Vesktop). Foregrounding
it then needs the `AttachThreadInput` dance, because Windows refuses
`SetForegroundWindow` from a process that doesn't own the foreground.

**A write can block behind a blocking read on the same pipe.** Windows
serialises I/O on a *file object*, and `try_clone` duplicates the handle
without creating a new one. The session thread lives in a blocking `ReadFile`,
so a command written from the UI thread could not start until that read
returned — which only happened when Discord next sent an event. The symptom was
that clicking mute did nothing until somebody started or stopped speaking. The
transport now opens the pipe with `FILE_FLAG_OVERLAPPED` and gives each
direction its own `OVERLAPPED` and event; writes went from blocking
indefinitely to 0.01 ms. `examples/pipe_concurrency_probe.rs` demonstrates both
behaviours.

**Reading the screen DC costs ~16 ms.** Sampling the taskbar colour forces the
compositor to read back from the GPU, and it was being done on the UI thread on
every single repaint — thousands of times more expensive than everything else a
refresh does (`taskbar::find()` is 0.005 ms). The readback costs the same
whether it is one `GetPixel` or a whole row via `BitBlt`, so the sample is
cached and only retaken when the taskbar changes or every ten seconds.

Sampling is also done defensively. The first version read three points and took
the first one, so a single unlucky pixel — a notification, a window overlapping
the bar — tinted the entire widget. It now takes the most common colour across
the whole bar and requires a clear majority before believing it, and re-checks
periodically so a bad sample cannot persist.

**A config the app cannot parse must not be fatal.** The release build has no
console, so exiting on a parse error means the app simply vanishes with no
indication why. A stray comma, or the byte-order mark that Notepad and
PowerShell's `Set-Content -Encoding utf8` prepend, was enough. The BOM is now
stripped, a broken config falls back to defaults with a visible notice, and the
file is deliberately *not* rewritten in that case — overwriting it would replace
the user's credentials with blanks over a typo.

**Layered windows work fine — as long as they are top-level, and you pass
`psize`.** The restriction that forced the widget to paint opaquely applies only
to *child* windows; the volume popup is top-level, so it uses
`UpdateLayeredWindow` and gets genuine per-pixel alpha with properly
anti-aliased rounded corners.

Getting there needed one more thing. `UpdateLayeredWindow` documents `psize` as
mandatory whenever `hdcSrc` is given, and passing `NULL` for it — intending
"keep the current size", with `SetWindowPos` having already set it — produced a
window that was created, visible, topmost, correctly sized and completely
blank. `examples/popup_probe.rs --live` puts a real popup on screen for
inspection, which is the only way to tell "rendered correctly" apart from
"presented correctly".

**A slider has to remember where it was left.** Once the drag ended, the bar
re-read the value the menu had been opened with and snapped back to it. Only
visually — the change had really been applied — but a control that visibly
reverts is indistinguishable from one that failed. The displayed value is now
the live drag position, else the last settled position, and only then the
value it opened with.

**A slider must not dismiss the thing containing it.** The volume bar first
returned a choice like any other menu item, so releasing the drag closed the
menu — which reads as the adjustment having been cancelled, even though it had
been applied. Continuous controls now keep the menu open and apply through a
callback as they move; only discrete commands close it. Applying during the
drag also means you hear the change while making it, rather than after.

**Hit-test against what was drawn, not against a second guess at it.** The
menu's volume bar sits between a label and a percentage, both of which are
measured text — so where it lands is not knowable from the metrics alone. The
first version estimated the bar's extent independently, and clicking set a
volume some distance from the one under the pointer. Rendering now reports the
bar's real extent and hit-testing uses that.

**A menu cannot rely on being the foreground window.**
`SetForegroundWindow` is refused when the calling process did not receive the
input that led there, so dismissing on foreground loss closed the menu within a
tick of opening it. Mouse capture is what actually decides whether a menu can
still see the pointer, so that is what the fallback check tests.

**`SetWindowPos` delivers `WM_PAINT` synchronously, and that paint can be
lost.** The host keeps its state in a `RefCell`, so a paint arriving *during* a
refresh cannot borrow it. `EndPaint` has already validated the region by then,
so the frame silently disappears and the widget shows stale state until
something else happens to invalidate it. The paint handler now reports whether
it actually painted, and the widget re-invalidates when it did not.

**`VOICE_CONNECTION_STATUS` is a trap.** It sounds useful, but Discord pushes it
every ~5 seconds carrying a full ping history — 6.8 KB per event, measured.
Deserialising that forever to display nothing is not worth it, so the app
doesn't subscribe to it.

### Adding a feature

The widget's contents are a list of `Element`s (`src/ui/elements.rs`). An element
measures itself and draws itself; layout, sizing and positioning are already
handled. Adding a ping readout, a screenshare marker or a participant count means
writing one `Element` and adding it to the list in `src/ui/host.rs`.

The data side is behind `StatusProvider`-shaped plumbing in `src/provider/`. The
UI consumes `ProviderEvent`s and doesn't know where they came from, so a second
provider (a Vencord plugin, say) can be added without touching `src/ui/`.

## Known limitations

- **Bottom-docked horizontal taskbars only.** Left/right-docked taskbars fall
  back to centring in the bar's long axis, which is unlikely to be what you
  want. Secondary-monitor taskbars are not handled.
- **Unread message count is not available.** Discord's RPC API has no command
  that returns a total mention count. The closest is counting
  `NOTIFICATION_CREATE` events under the `rpc.notifications.read` scope and
  resetting when Discord takes foreground, which drifts if you read messages on
  another device. A Vencord provider would read the real number directly.
- **Opaque background.** Because layered child windows aren't composited, the
  widget paints the taskbar colour it sampled. On a solid taskbar this is
  invisible; on a translucent or acrylic taskbar it would not blend perfectly.

## Server moderation: what it would take

Neither route is implemented, and neither should be until you have decided
which trade-off you want.

### A companion bot — the supported route

A bot of yours, invited to the guild with Mute Members / Deafen Members / Move
Members, performs the action; the desktop app keeps using RPC for local voice
state and asks the bot to do the rest.

This is the only route Discord actually sanctions. It also gets you correct
permission checking (a bot token can read channel overwrites and compute
effective permissions, which OAuth2 cannot) and audit-log entries via
`X-Audit-Log-Reason`. The costs are a bot to host, an invite per server, and
one design problem worth naming: the *bot's* permissions gate the action, not
yours, so the app has to check the clicking user's permissions itself or it
becomes a privilege-escalation tool.

### A Vencord plugin — works, but it is self-botting

A plugin runs inside Discord's client and can call `RestAPI.patch` with the
user's own session, exactly as the official client does. It can also check
permissions properly, which is the one genuine advantage:
`PermissionStore.can(PermissionsBits.MOVE_MEMBERS, channel)` is
overwrite-aware and channel-scoped.

The existing example is `VoiceChatUtilities`, which does precisely this. Two
things are worth knowing before going down this path. It was **rejected from
Vencord** — the maintainer closed the PR saying "this plugin is essentially a
selfbot" and pointed at a bot as the right answer, so it lives on as a
user-plugin. And Discord's developer docs state plainly that developers must
refrain "from automating standard user accounts (generally called 'self-bots')
outside of the OAuth2/bot API".

Whether one deliberate click counts as "automating" is genuinely ambiguous and
Discord has never clarified it, despite being asked directly more than once.
In practice client mods are not enforced against — but that is observed
non-enforcement, not permission, and the account at risk would be yours.

### What is possible today, cheaply

The `guilds` OAuth2 scope would let the app read your **guild-level**
permissions from `GET /users/@me/guilds` and grey out moderation UI you
clearly cannot use. It ignores channel overwrites in both directions, so it is
good enough to hide buttons and not good enough to authorize anything.

## Demo mode

```sh
./target/release/discord-taskbar.exe --demo
```

Drives the whole interface from a synthetic call — three real avatars, someone
always talking, working scroll and click — without needing to be in a voice
channel. It runs under its own single-instance mutex, so it can sit alongside
the real one.

This exists because every interface change used to need a live call to look at,
which made some paths awkward to check and others only reasonable about. It is
the fastest way to work on anything visual.

## When nothing shows up

The widget takes up no space when there is nothing to say — not in a call,
Discord not running, no credentials yet. That is right in normal use and
unhelpful when something is wrong, because every failure looks the same from
outside: an empty taskbar.

```sh
discord-taskbar.exe --doctor
```

or **Diagnostics...** in the tray menu. It runs every check the app makes at
startup and shows the result in a window you can copy or save. It changes
nothing — no OAuth prompt, no config rewrite, no widget — and works while the
app is already running, because it runs ahead of the single-instance check.

It covers where the exe is running from, whether `config.json` parses, whether
the client id looks like a client id, whether a usable token is cached and
which scopes it has, whether a `discord-ipc-N` pipe is answering, and — the
useful one — it performs a real RPC handshake. That validates the client id
against Discord without triggering an authorisation prompt, and reports which
account is signed in, which is how you catch the most common mistake: using
somebody else's client id. Discord restricts the `rpc` scope to the
application's **owner**, so a client id copied from a friend can never work.

Finally it reports every taskbar found, with its geometry, and whether
`ReBarWindow32` and `TrayNotifyWnd` are present inside `Shell_TrayWnd` — their
absence is the signature of a stock Windows 11 taskbar, which is not the
classic hierarchy this relies on.

The client secret is never printed, only measured, so the report is safe to
paste into a chat window.

## Diagnostics

| | |
|---|---|
| `cargo run --example rpc_probe` | Connect to Discord and dump live voice events. The first thing to run if the widget isn't updating. |
| `cargo run --example parent_probe` | Report which taskbar-parenting strategies work, with integrity levels. |
| `cargo run --example render_probe` | Render the widget's content to a buffer, to separate drawing bugs from compositing bugs. |
| `cargo run --example layout_probe` | Render the whole widget for every state — speaking, muted, deafened, server-muted — from synthetic data, without needing a live call to reproduce them. |
| `cargo run --example icon_probe` | Render every glyph at every size in use. The icons live at 9–20 px; change anything about them and look at this before believing it works. |
| `cargo run --example glyph_probe` | Dump labelled code points from the Windows icon fonts, for picking new glyphs. |
| `cargo run --release --example stress_probe` | Render 5,000 frames and report private bytes, handles and GDI objects. Anything that allocates per repaint shows up here. |
| `cargo run --example voice_probe` | Time `SET_VOICE_SETTINGS` end to end — Discord's acknowledgement and the confirming event. Run this first if mute feels slow, to tell Discord's latency apart from ours. |
| `cargo run --release --example paint_timing_probe` | Time the individual steps of a refresh. |
| `cargo run --release --example slash_probe` | Recover the slash geometry baked into the font's `MicOff` glyph, so a hand-drawn one can match it. |
| `./build.sh` | Build **and check it worked**. `cargo build --examples` does not build the `[[bin]]` target, and a running instance locks the exe — either will leave a stale binary behind while looking like success. This checks the exit status and prints the binary's timestamp. |
| `cargo run --release --example monitor_probe` | List every taskbar the app can attach to, with its display and geometry. |
| `cargo run --release --example menu_probe` | Show the drawn user menu on its own, which separates "does it render" from "does a click reach it". |
| `cargo run --release --example popup_probe` | Render the volume popup at several levels over a chequerboard, to check the alpha and the rounded corners. Add `-- --live` to put a real popup on screen instead. |
| `cargo run --release --example pipe_concurrency_probe` | Check that a write can proceed while a read is parked. Writes taking longer than a millisecond mean the transport has regressed to synchronous I/O. |

## Layout

```
src/
  config.rs              config.json load/save
  http.rs                minimal HTTPS over WinHTTP
  model.rs               VoiceStatus / Participant — provider-agnostic
  provider/
    mod.rs               ProviderEvent, ProviderSink, ProviderControl
    rpc/
      pipe.rs            named-pipe transport and frame codec
      mod.rs             RpcClient: commands, events, nonce matching
      oauth.rs           AUTHORIZE → token → AUTHENTICATE, token cache
      events.rs          subscription bookkeeping
      session.rs         event loop folding events into VoiceStatus
  ui/
    host.rs              hidden owner window, state, timer
    widget.rs            the taskbar child window
    taskbar.rs           Shell_TrayWnd discovery, geometry, colour sampling
    render.rs            DIB canvas, compositing, text, shapes
    elements.rs          Element trait and the built-in elements
    theme.rs             colours and metrics
    tray.rs              notification icon and menu
  assets/
    avatars.rs           WinHTTP fetch + WIC decode + two-level cache
    icons.rs             procedurally drawn mic/headphone glyphs
```
