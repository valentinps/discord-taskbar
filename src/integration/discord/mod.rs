//! Discord voice status.
//!
//! The reference integration: it reads voice state over Discord's local RPC
//! pipe and describes it as blocks the widget draws. Everything specific to
//! Discord lives under here — the pipe, OAuth, the voice model, the volume
//! curve and the decisions about what a click means.

pub mod icons;
pub mod model;
pub mod provider;
pub mod view;
pub mod volume;

use std::time::Duration;

use crate::assets::icons::Glyph;
use crate::config::Credentials;
use crate::ui::block::{Block, BlockId};
use crate::ui::integration::{
    Event, EventSink, Gesture, Integration, Interaction, Meter, TrayItem, Ui, TRAY_ID_BASE,
};
use crate::ui::menu;
use crate::ui::render::Color;
use crate::ui::theme::{MiddleClick, Theme};

use model::VoiceStatus;
use provider::{ProviderControl, ProviderEvent};

/// Where Discord serves avatars and server icons.
pub const CDN_HOST: &str = "cdn.discordapp.com";

/// Discord blurple, so the tray icon is recognisable at a glance.
const BRAND: Color = Color::rgb(0x58, 0x65, 0xF2);

/// How long a refused command stays on the widget.
const NOTICE_LINGER: Duration = Duration::from_secs(4);

/// Minimum time the volume readout stays up when it was not opened by
/// hovering — after a menu choice the pointer is nowhere near the avatar.
const METER_HOLD: Duration = Duration::from_millis(1200);

// Tray commands. Above `TRAY_ID_BASE`, which is where the widget's own end.
const CMD_FOCUS: usize = TRAY_ID_BASE;
const CMD_RECONNECT: usize = TRAY_ID_BASE + 1;
const CMD_REAUTHORIZE: usize = TRAY_ID_BASE + 2;

// Rows in the per-participant menu.
const ID_LOCAL_MUTE: usize = 1;
const ID_FOCUS_FROM_MENU: usize = 2;

pub struct Discord {
    status: VoiceStatus,
    control: ProviderControl,
    creds: Credentials,
    /// Drive the widget from a synthetic call instead of the real client, so
    /// interface work does not require being in a voice channel.
    demo: bool,
}

impl Discord {
    pub fn new(creds: Credentials, demo: bool) -> Self {
        Discord {
            status: VoiceStatus::default(),
            control: if demo {
                ProviderControl::demo()
            } else {
                ProviderControl::new()
            },
            creds,
            demo,
        }
    }

    /// Mirror the Discord client's coupling: clicking the microphone while
    /// deafened lifts both, because a muted-but-not-deafened state is what the
    /// user is actually asking for.
    fn toggle_mute(&mut self, ui: &mut dyn Ui) {
        let state = self.status.self_state;
        let (mute, deaf) = if state.deaf {
            (false, false)
        } else {
            (!state.mute, state.deaf)
        };
        self.apply_voice(mute, deaf, ui);
    }

    fn toggle_deafen(&mut self, ui: &mut dyn Ui) {
        let deaf = !self.status.self_state.deaf;
        // Deafening mutes; undeafening restores an unmuted mic, which is what
        // the Discord client does.
        self.apply_voice(deaf, deaf, ui);
    }

    /// Send the change and reflect it immediately.
    ///
    /// Discord acknowledges in tens of milliseconds, but waiting for the round
    /// trip before redrawing makes the button feel broken. The optimistic
    /// state is replaced by whatever `VOICE_SETTINGS_UPDATE` reports, so if the
    /// write is refused the widget snaps back rather than lying.
    fn apply_voice(&mut self, mute: bool, deaf: bool, ui: &mut dyn Ui) {
        if self.control.set_voice(Some(mute), Some(deaf)) {
            self.status.self_state.mute = mute;
            self.status.self_state.deaf = deaf;
            if let Some(me) = self.status.participants.iter_mut().find(|p| p.is_self) {
                me.self_mute = mute;
                me.self_deaf = deaf;
            }
        } else {
            ui.notice("Not connected to Discord".to_string(), Some(NOTICE_LINGER));
        }
        ui.redraw();
    }

    fn toggle_local_mute(&mut self, user_id: &str, ui: &mut dyn Ui) {
        let Some(participant) = self.status.participants.iter().find(|p| p.user_id == user_id)
        else {
            return;
        };
        let muted = !participant.local_mute;

        if self.control.set_user_voice(user_id, None, Some(muted)) {
            if let Some(participant) = self.status.participant_mut(user_id) {
                participant.local_mute = muted;
            }
            ui.redraw();
        }
    }

    /// Set someone's local volume and raise the readout.
    ///
    /// `shown` is what the user sees: a perceptual percentage, as on Discord's
    /// own slider.
    fn set_user_volume(&mut self, user_id: &str, shown: f32, hold: Duration, ui: &mut dyn Ui) {
        let theme = ui.theme().clone();
        let amplitude = theme.stored_volume(shown);
        if !self.control.set_user_voice(user_id, Some(amplitude), None) {
            return;
        }

        let name = self
            .status
            .participants
            .iter()
            .find(|p| p.user_id == user_id)
            .map(|p| p.display_name.clone())
            .unwrap_or_default();

        if let Some(participant) = self.status.participant_mut(user_id) {
            participant.volume = amplitude;
        }

        self.raise_meter(user_id, &name, shown, hold, &theme, ui);
        // Redraw too: dropping to zero dims the avatar. The layout is
        // unchanged, so the pointer stays over the same person and the next
        // notch lands where this one did.
        ui.redraw();
    }

    fn raise_meter(
        &self,
        user_id: &str,
        name: &str,
        shown: f32,
        hold: Duration,
        theme: &Theme,
        ui: &mut dyn Ui,
    ) {
        let image = self
            .status
            .participants
            .iter()
            .find(|p| p.user_id == user_id)
            .map(|p| p.avatar_ref(theme.avatar_size as u32));

        ui.show_meter(Meter {
            about: BlockId::new(format!("{}:{user_id}", view::ID_USER)),
            title: name.to_string(),
            value_text: view::percent(shown),
            fraction: shown / theme.volume_ceiling(shown),
            // Past 100% is boosting, which is worth saying in colour.
            fill: if shown > 100.5 {
                theme.danger
            } else {
                theme.speaking
            },
            image,
            image_size: theme.avatar_size,
            hold,
        });
    }

    /// Wheel over a participant adjusts how loud they are, locally.
    fn on_wheel(&mut self, user_id: &str, notches: i32, ui: &mut dyn Ui) {
        let Some(participant) = self.status.participants.iter().find(|p| p.user_id == user_id)
        else {
            return;
        };
        // Your own volume is not a thing Discord lets you set.
        if participant.is_self {
            return;
        }

        let theme = ui.theme().clone();
        // A notch moves the number the user can see, which is Discord's
        // perceptual percentage — not the amplitude underneath it. Stepping
        // the amplitude instead would move the slider by wildly different
        // amounts depending on where it already was.
        let shown = theme.shown_volume(participant.volume);
        let ceiling = theme.volume_ceiling(shown);
        let next = (shown + notches as f32 * theme.scroll_volume_step as f32).clamp(0.0, ceiling);

        // Opened by the wheel, so the pointer is already on the avatar and
        // moving off it should dismiss immediately.
        self.set_user_volume(user_id, next, Duration::ZERO, ui);
    }

    /// Per-participant menu, drawn in the widget's own style.
    ///
    /// Only local actions appear. Server mute, server deafen and disconnecting
    /// somebody else have no Discord RPC command — see the README.
    ///
    /// Loops rather than returning after one menu: dismissing by clicking
    /// another participant should move the panel to them, and clicking the
    /// same one again should just close it.
    fn show_user_menu(&mut self, user_id: &str, ui: &mut dyn Ui) {
        // The panel supersedes the small readout; leaving both up would show
        // the same number twice.
        ui.hide_meter();

        let mut target = user_id.to_string();

        loop {
            let Some(participant) = self
                .status
                .participants
                .iter()
                .find(|p| p.user_id == target)
                .cloned()
            else {
                return;
            };

            let theme = ui.theme().clone();
            // Everything in the menu is the perceptual percentage Discord's
            // own slider shows, not the amplitude underneath it.
            let shown = theme.shown_volume(participant.volume);

            let items = vec![
                menu::Item::Header {
                    name: participant.display_name.clone(),
                    image: Some(participant.avatar_ref(theme.avatar_size as u32)),
                    image_size: theme.avatar_size,
                },
                menu::Item::Separator,
                menu::Item::Action {
                    id: ID_LOCAL_MUTE,
                    label: if participant.local_mute {
                        "Unmute for me".to_string()
                    } else {
                        "Mute for me".to_string()
                    },
                    icon: Some(if participant.local_mute {
                        icons::VOLUME_MUTED
                    } else {
                        icons::VOLUME
                    }),
                    checked: participant.local_mute,
                    danger: participant.local_mute,
                },
                menu::Item::Slider {
                    label: "Volume".to_string(),
                    value: shown,
                    range: (0.0, theme.volume_ceiling(shown)),
                    format: view::percent,
                    warn_above: Some(100.5),
                },
                menu::Item::SliderPreset {
                    label: "Reset volume".to_string(),
                    value: 100.0,
                },
                menu::Item::Separator,
                menu::Item::Action {
                    id: ID_FOCUS_FROM_MENU,
                    label: "Focus Discord".to_string(),
                    icon: None,
                    checked: false,
                    danger: false,
                },
            ];

            // Applied from inside the menu while the bar is dragged, so it is
            // audible immediately and the menu stays open. The control is
            // cheap to clone and borrows nothing else here.
            let control = self.control.clone();
            let applying_to = target.clone();
            let curve = (theme.volume_curve, theme.volume_boost_db);
            let apply = move |shown: f32| {
                // The bar hands back a perceptual percentage; Discord wants
                // the amplitude.
                let amplitude = volume::perceptual_to_amplitude(shown, curve.0, curve.1);
                control.set_user_voice(&applying_to, Some(amplitude), None);
            };

            let outcome = ui.show_menu(&items, Some(&apply));

            // Reconcile our own copy with whatever the bar was left at;
            // Discord's own event will confirm it shortly.
            if let Some(shown) = outcome.slider {
                if let Some(participant) = self.status.participant_mut(&target) {
                    participant.volume = theme.stored_volume(shown);
                }
                ui.redraw();
            }

            match outcome.choice {
                Some(menu::Choice::Action(ID_LOCAL_MUTE)) => {
                    self.toggle_local_mute(&target, ui);
                    return;
                }
                Some(menu::Choice::Action(ID_FOCUS_FROM_MENU)) => {
                    focus();
                    return;
                }
                Some(_) => return,
                None => {}
            }

            // Dismissed by clicking outside. If that click was on a different
            // participant, show theirs; on the same one, it was a toggle.
            let Some(screen) = outcome.dismissed_at else {
                return;
            };
            let Some(block) = ui.block_at(screen) else {
                return;
            };
            let Some(next) = view::user_of(&block) else {
                return;
            };
            if next == target {
                return;
            }
            target = next.to_string();
        }
    }
}

impl Integration for Discord {
    fn id(&self) -> &'static str {
        "discord"
    }

    fn name(&self) -> &str {
        "Discord"
    }

    fn tray_icon(&self) -> (Glyph, Color) {
        (icons::HEADPHONES, BRAND)
    }

    fn start(&mut self, events: EventSink) {
        if self.demo {
            provider::spawn_demo(events);
        } else {
            provider::spawn_rpc(self.creds.clone(), events, self.control.clone());
        }
    }

    fn on_event(&mut self, event: Event, ui: &mut dyn Ui) -> bool {
        let Ok(event) = event.downcast::<ProviderEvent>() else {
            return false;
        };

        match *event {
            ProviderEvent::Status(status) => {
                ui.clear_notice();
                self.status = (*status).clone();
            }
            ProviderEvent::AwaitingAuthorization => {
                ui.notice("Authorize in Discord".to_string(), None);
            }
            ProviderEvent::CommandFailed(message) => {
                // Most likely another RPC client holds Discord's voice-settings
                // lock. Show it briefly rather than appearing to ignore a click.
                ui.notice(message, Some(NOTICE_LINGER));
            }
            ProviderEvent::Offline(reason) => {
                // Discord simply not running is the normal idle case, not
                // something worth putting on the taskbar.
                if reason.contains("not running") {
                    ui.clear_notice();
                } else {
                    ui.notice(reason, None);
                }
            }
        }
        true
    }

    fn blocks(&self, theme: &Theme) -> Vec<Block> {
        view::blocks(&self.status, theme)
    }

    fn tooltip(&self) -> String {
        if self.status.is_connected() {
            format!("Discord Taskbar — {}", self.status.location_label())
        } else {
            "Discord Taskbar — not in voice".to_string()
        }
    }

    fn on_interaction(&mut self, interaction: Interaction, ui: &mut dyn Ui) {
        let Some(id) = interaction.target.clone() else {
            return;
        };

        match interaction.gesture {
            Gesture::Click => match id.as_str() {
                view::ID_MUTE => self.toggle_mute(ui),
                view::ID_DEAFEN => self.toggle_deafen(ui),
                view::ID_LEAVE => {
                    self.control.leave_voice();
                }
                // Only the server icon and the channel name jump to Discord;
                // clicking a participant is for acting on that participant.
                view::ID_FOCUS => {
                    focus();
                }
                _ => {
                    if let Some(user_id) = view::user_of(&id) {
                        let user_id = user_id.to_string();
                        self.show_user_menu(&user_id, ui);
                    }
                }
            },

            Gesture::Wheel(notches) => {
                if let Some(user_id) = view::user_of(&id) {
                    let user_id = user_id.to_string();
                    self.on_wheel(&user_id, notches, ui);
                }
            }

            Gesture::Middle => {
                let Some(user_id) = view::user_of(&id).map(str::to_string) else {
                    return;
                };
                match ui.theme().middle_click {
                    MiddleClick::None => {}
                    MiddleClick::LocalMute => self.toggle_local_mute(&user_id, ui),
                    MiddleClick::VolumeReset => {
                        self.set_user_volume(&user_id, 100.0, METER_HOLD, ui)
                    }
                }
            }

            Gesture::Hover | Gesture::Leave => {}
        }
    }

    fn tray_items(&self) -> Vec<TrayItem> {
        vec![
            TrayItem::command(CMD_FOCUS, "Focus Discord").enabled(self.status.is_connected()),
            TrayItem::command(CMD_RECONNECT, "Reconnect"),
            TrayItem::command(CMD_REAUTHORIZE, "Re-authorize with Discord"),
        ]
    }

    fn on_tray_command(&mut self, id: usize, _ui: &mut dyn Ui) {
        match id {
            CMD_FOCUS => {
                focus();
            }
            CMD_RECONNECT => self.control.reconnect(),
            CMD_REAUTHORIZE => self.control.reauthorize(),
            _ => {}
        }
    }
}

/// Bring the Discord window forward.
pub fn focus() -> bool {
    crate::ui::tray::focus_discord()
}
