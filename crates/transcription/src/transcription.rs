use async_channel::{Receiver, Sender};
use gpui::{actions, App, BorrowAppContext, Global, Subscription, UpdateGlobal};
use log::{error, info, warn};
use settings::Settings;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Condvar, Mutex, Weak};

use transcription_settings::SpeechSettings;

mod thread_loop;

actions!(
    transcription,
    [
        /// Toggles the speech recognizer on and off.
        ToggleDictationChannel
    ]
);

#[derive(Clone, Debug)]
pub enum TranscriptionNotification {
    ModelNotFound(String),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TranscriptionThreadState {
    Disabled,
    Idle,
    Listening,
    Transcribing,
}

pub struct TranscriptionNotificationStream {
    receiver: Receiver<TranscriptionNotification>,
}

impl TranscriptionNotificationStream {
    fn new(receiver: Receiver<TranscriptionNotification>) -> Self {
        Self { receiver }
    }

    pub async fn recv(&mut self) -> Option<TranscriptionNotification> {
        self.receiver.recv().await.ok()
    }
}

#[derive(Default)]
pub(crate) struct TranscriptionThreadController {
    pub kill: AtomicBool,
    pub interested: (Mutex<usize>, Condvar),
    pub finish_up: AtomicBool,
}

pub struct InterestGuard(Arc<TranscriptionThreadController>);
impl Drop for InterestGuard {
    fn drop(&mut self) {
        self.0.decrease_interest();
    }
}

impl TranscriptionThreadController {
    pub fn kill(&self) {
        self.kill.store(true, std::sync::atomic::Ordering::SeqCst);
    }

    fn increase_interest(&self) {
        *self.interested.0.lock().unwrap() += 1;
        self.interested.1.notify_one();
    }

    fn decrease_interest(&self) {
        *self.interested.0.lock().unwrap() -= 1;
        // no need to notify here, the thread only cares if the number of interested parties goes up.
    }

    pub fn interested(self: Arc<Self>) -> InterestGuard {
        self.increase_interest();
        InterestGuard(self)
    }

    pub fn interest(&self) -> usize {
        *self.interested.0.lock().unwrap()
    }

    pub fn wait_until_interest(&self) {
        let lock = self.interested.0.lock().unwrap();
        let _lock = self
            .interested
            .1
            .wait_while(lock, |interested| *interested == 0)
            .unwrap();
    }
}

pub struct Transcription {
    state: TranscriptionThreadState,

    task: Option<(
        Arc<TranscriptionThreadController>,
        std::thread::JoinHandle<()>,
    )>,

    transcription_sender: Sender<String>,
    notification_sender: Sender<TranscriptionNotification>,
    state_change_sender: Sender<TranscriptionThreadState>,

    notification_subscribers: Vec<Sender<TranscriptionNotification>>,
    transcription_subscribers: Vec<Weak<dyn Fn(String, &mut App) + Send>>,
}

impl Global for Transcription {}

impl Transcription {
    pub fn state(&self) -> TranscriptionThreadState {
        self.state
    }

    fn new(cx: &mut App) -> Self {
        info!("Initializing speech global");
        let (transcription_sender, transcription_receiver) = async_channel::unbounded::<String>();
        let (notification_sender, notification_receiver) =
            async_channel::unbounded::<TranscriptionNotification>();
        let notification_subscribers = Vec::new();
        let transcription_subscribers = Vec::new();

        {
            cx.spawn(async move |cx| {
                while let Ok(notification) = notification_receiver.recv().await {
                    cx.update_global(|transcription: &mut Self, _| {
                        transcription
                            .notification_subscribers
                            .retain(|subscriber| subscriber.try_send(notification.clone()).is_ok());
                    });
                    #[allow(irrefutable_let_patterns)] // More notifications to come
                    if let TranscriptionNotification::ModelNotFound(path) = notification {
                        warn!("Speech model not found at: {path}");
                    }
                }
            })
            .detach();
        }

        {
            cx.spawn(async move |cx| {
                while let Ok(text) = transcription_receiver.recv().await {
                    let text = text.clone();
                    cx.update(|cx| {
                        cx.update_global(|transcription: &mut Self, cx| {
                            transcription
                                .transcription_subscribers
                                .retain(|cb| cb.upgrade().map(|cb| cb(text.clone(), cx)).is_some())
                        })
                    });
                }
            })
            .detach();
        }

        let (state_change_sender, state_change_receiver) = async_channel::unbounded();
        {
            cx.spawn(async move |cx| {
                while let Ok(state) = state_change_receiver.recv().await {
                    cx.update_global(|g: &mut Self, _| {
                        g.state = state;
                    });
                }
            })
            .detach();
        }

        let mut this = Self {
            state: TranscriptionThreadState::Disabled,
            task: None,
            transcription_sender,
            notification_sender,
            notification_subscribers,
            transcription_subscribers,
            state_change_sender,
        };
        this.start_thread(cx);
        this
    }

    pub fn subscribe(
        &mut self,
        callback: impl Fn(String, &mut App) + Send + 'static,
    ) -> Subscription {
        let cb = Arc::new(callback) as _;
        self.transcription_subscribers.push(Arc::downgrade(&cb));

        let interest = self.task.as_ref().unwrap().0.clone().interested();

        Subscription::new(move || {
            drop(interest);
            drop(cb);
        })
    }

    pub fn subscribe_notifications(&mut self) -> TranscriptionNotificationStream {
        let (sender, receiver) = async_channel::unbounded();
        self.notification_subscribers.push(sender);
        TranscriptionNotificationStream::new(receiver)
    }

    fn toggle_listening(&mut self, cx: &mut App) {
        if let Some((controller, handle)) = self.task.take() {
            controller.kill();
            // wake the thread if it's sleeping
            let _interest = controller.interested();
            handle
                .join()
                .unwrap_or_else(|_| warn!("Failed to join speech thread"));
            self.state = TranscriptionThreadState::Disabled;
            info!("Speech listening stopped");
        } else {
            self.start_thread(cx);
            info!("Speech listening started");
        }
    }

    fn start_thread(&mut self, cx: &mut App) {
        self.state = TranscriptionThreadState::Idle;

        let transcription_sender = self.transcription_sender.clone();
        let notification_sender = self.notification_sender.clone();
        let state_change_sender = self.state_change_sender.clone();
        let controller = Arc::new(TranscriptionThreadController::default());
        let task = Transcription::run_transcription_loop(
            controller.clone(),
            transcription_sender,
            notification_sender,
            state_change_sender,
            cx,
        );
        self.task = Some((controller, task));
    }

    fn run_transcription_loop(
        controller: Arc<TranscriptionThreadController>,
        transcription_sender: Sender<String>,
        notification_sender: Sender<TranscriptionNotification>,
        state_change_sender: Sender<TranscriptionThreadState>,
        cx: &mut App,
    ) -> std::thread::JoinHandle<()> {
        info!("Launching transcription loop");
        let settings = SpeechSettings::get_global(cx).clone();

        std::thread::spawn(move || {
            if let Err(err) = thread_loop::transcription_loop_body(
                settings,
                controller,
                transcription_sender,
                notification_sender,
                state_change_sender,
            ) {
                error!("error in transcription loop: {}", err);
            }
        })
    }

    pub fn finish_current(&self) {
        if let Some((controller, _)) = &self.task {
            controller
                .finish_up
                .store(true, std::sync::atomic::Ordering::SeqCst);
        } else {
            warn!("Tried to finish current transcription early, but the thread is disabled")
        }
    }
}

pub fn init(cx: &mut App) {
    let speech = Transcription::new(cx);
    cx.set_global(speech);

    cx.on_action(|_: &ToggleDictationChannel, cx| {
        Transcription::update_global(cx, |speech, cx| {
            speech.toggle_listening(cx);
        });
    });
}
