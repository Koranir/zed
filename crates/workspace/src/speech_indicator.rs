use crate::StatusItemView;
use gpui::{Action, Context, IntoElement, Render, Subscription, Window};
use theme::ActiveTheme;
use transcription::{ToggleDictationChannel, Transcription, TranscriptionThreadState};
use ui::Clickable;

pub struct SpeechIndicator {
    subscription: Option<Subscription>,
    state: TranscriptionThreadState,
}

impl SpeechIndicator {
    pub fn new() -> Self {
        Self {
            subscription: None,
            state: TranscriptionThreadState::Idle,
        }
    }
}

impl Render for SpeechIndicator {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.subscription.is_none() {
            self.subscription = Some(cx.observe_global::<Transcription>(|this, cx| {
                let speech_state = cx.global::<Transcription>().state();
                if this.state != speech_state {
                    this.state = speech_state;
                    cx.notify();
                }
            }));
        }

        let color = match self.state {
            TranscriptionThreadState::Idle => cx.theme().colors().icon_muted,
            TranscriptionThreadState::Transcribing => cx.theme().colors().icon_accent,
            _ => cx.theme().colors().icon,
        };

        ui::IconButton::new(
            "speech-indicator",
            match self.state {
                TranscriptionThreadState::Disabled => ui::IconName::MicMute,
                TranscriptionThreadState::Idle => ui::IconName::Mic,
                TranscriptionThreadState::Listening => ui::IconName::Mic,
                TranscriptionThreadState::Transcribing => ui::IconName::Mic,
            },
        )
        .icon_color(color.into())
        .on_click(cx.listener(|_, _, window, cx| {
            window.dispatch_action(ToggleDictationChannel.boxed_clone(), cx);
        }))
    }
}

impl StatusItemView for SpeechIndicator {
    fn set_active_pane_item(
        &mut self,
        _active_pane_item: Option<&dyn crate::ItemHandle>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) {
        // Not needed for this indicator
    }
}
