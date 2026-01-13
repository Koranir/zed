use async_channel::{Receiver, Sender};
use gpui::{actions, App, Global, Subscription, UpdateGlobal};
use log::{error, info, warn};
use settings::Settings;
use std::collections::BTreeMap;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

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
    pub wait: AtomicBool,
}

pub struct Transcription {
    state: TranscriptionThreadState,
    task: Option<(
        Arc<TranscriptionThreadController>,
        std::thread::JoinHandle<()>,
    )>,
    transcription_sender: Sender<String>,
    notification_sender: Sender<TranscriptionNotification>,
    notification_subscribers: Arc<std::sync::Mutex<Vec<Sender<TranscriptionNotification>>>>,
    transcription_subscribers:
        Arc<std::sync::Mutex<BTreeMap<usize, Box<dyn FnMut(String, &mut App) -> bool + Send>>>>,
    next_subscriber_id: usize,
    state_change_sender: Sender<TranscriptionThreadState>,
}

impl Global for Transcription {}

impl Transcription {
    pub fn state(&self) -> TranscriptionThreadState {
        self.state
    }

    fn new(cx: &mut App) -> Self {
        info!("Initializing speech global");
        let (transcription_sender, transcription_receiver) = async_channel::unbounded::<String>();
        let (notification_sender, notification_receiver) = async_channel::unbounded();
        let notification_subscribers = Arc::new(std::sync::Mutex::new(Vec::new()));
        let transcription_subscribers = Arc::new(std::sync::Mutex::new(BTreeMap::<
            usize,
            Box<dyn FnMut(String, &mut App) -> bool + Send>,
        >::new()));

        {
            let notifications: Receiver<TranscriptionNotification> = notification_receiver.clone();
            let subscribers = notification_subscribers.clone();
            cx.spawn(async move |_| {
                while let Ok(notification) = notifications.recv().await {
                    Self::broadcast(&subscribers, notification.clone());
                    #[allow(irrefutable_let_patterns)] // More notifications to come
                    if let TranscriptionNotification::ModelNotFound(path) = notification {
                        warn!("Speech model not found at: {path}");
                    }
                }
            })
            .detach();
        }

        {
            let task_subscribers = transcription_subscribers.clone();
            let transcription_receiver = transcription_receiver.clone();
            cx.spawn(async move |cx| {
                while let Ok(text) = transcription_receiver.recv().await {
                    if task_subscribers.lock().unwrap().is_empty() {
                        continue;
                    }

                    let text = text.clone();
                    cx.update(|cx| {
                        let mut subscribers = task_subscribers.lock().unwrap();
                        for (_, callback) in subscribers.iter_mut() {
                            if callback(text.clone(), cx) {
                                break;
                            }
                        }
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

        Self {
            state: TranscriptionThreadState::Idle,
            task: None,
            transcription_sender,
            notification_sender,
            notification_subscribers,
            transcription_subscribers,
            next_subscriber_id: 0,
            state_change_sender,
        }
    }

    pub fn subscribe(
        &mut self,
        callback: impl FnMut(String, &mut App) -> bool + Send + 'static,
    ) -> Subscription {
        let id = self.next_subscriber_id;
        self.next_subscriber_id += 1;
        self.transcription_subscribers
            .lock()
            .unwrap()
            .insert(id, Box::new(callback));

        let subscribers = self.transcription_subscribers.clone();
        Subscription::new(move || {
            subscribers.lock().unwrap().remove(&id);
        })
    }

    pub fn subscribe_notifications(&self) -> TranscriptionNotificationStream {
        let (sender, receiver) = async_channel::unbounded();
        self.notification_subscribers.lock().unwrap().push(sender);
        TranscriptionNotificationStream::new(receiver)
    }

    fn broadcast<T: Clone>(subscribers: &Arc<std::sync::Mutex<Vec<Sender<T>>>>, value: T) {
        let mut sinks = subscribers.lock().unwrap();
        sinks.retain(|subscriber: &Sender<T>| subscriber.try_send(value.clone()).is_ok());
    }

    fn toggle_listening(&mut self, cx: &mut App) {
        if let Some(thread_handle) = self.task.take() {
            thread_handle
                .0
                .kill
                .store(true, std::sync::atomic::Ordering::SeqCst);
            thread_handle
                .1
                .join()
                .unwrap_or_else(|_| warn!("Failed to join speech thread"));
            self.state = TranscriptionThreadState::Disabled;
            info!("Speech listening stopped");
        } else {
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
            info!("Speech listening started");
        }
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
