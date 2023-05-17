use futures::{stream, StreamExt};
use std::collections::hash_map::Entry;
use std::collections::HashMap;
use std::fmt::Debug;
use std::hash::Hash;
use std::ops::{Deref, DerefMut};
use std::sync::Arc;
use tokio::sync::{mpsc, RwLock};
use tracing::debug;

#[derive(Clone, Debug)]
pub enum Event<K, V>
where
    K: Clone + Debug + Eq + Hash,
    V: Clone + Debug,
{
    Added(K, V),
    Modified(K, V),
    Removed(K),
    Restart(HashMap<K, V>),
}

pub struct Subscription<K, V>
where
    K: Clone + Debug + Eq + Hash + Send + Sync + 'static,
    V: Clone + Debug + Send + Sync + 'static,
{
    rx: mpsc::Receiver<Event<K, V>>,
    unsubscribe: Option<Box<dyn FnOnce() + Send + Sync + 'static>>,
}

impl<K, V> Subscription<K, V>
where
    K: Clone + Debug + Eq + Hash + Send + Sync,
    V: Clone + Debug + Send + Sync,
{
    pub fn new(
        rx: mpsc::Receiver<Event<K, V>>,
        unsubscribe: impl FnOnce() + Send + Sync + 'static,
    ) -> Self {
        Self {
            rx,
            unsubscribe: Some(Box::new(unsubscribe)),
        }
    }
}

impl<K, V> Drop for Subscription<K, V>
where
    K: Clone + Debug + Eq + Hash + Send + Sync + 'static,
    V: Clone + Debug + Send + Sync + 'static,
{
    fn drop(&mut self) {
        if let Some(unsubscribe) = self.unsubscribe.take() {
            unsubscribe();
        }
    }
}

impl<K, V> Deref for Subscription<K, V>
where
    K: Clone + Debug + Eq + Hash + Send + Sync,
    V: Clone + Debug + Send + Sync,
{
    type Target = mpsc::Receiver<Event<K, V>>;

    fn deref(&self) -> &Self::Target {
        &self.rx
    }
}

impl<K, V> DerefMut for Subscription<K, V>
where
    K: Clone + Debug + Eq + Hash + Send + Sync,
    V: Clone + Debug + Send + Sync,
{
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.rx
    }
}

#[derive(Clone, Debug)]
pub struct State<K, V>
where
    K: Clone + Debug + Eq + Hash,
    V: Clone + Debug + PartialEq,
{
    inner: Arc<RwLock<Inner<K, V>>>,
}

#[derive(Debug)]
struct Inner<K, V>
where
    K: Clone + Debug + Eq + Hash,
    V: Clone + Debug + PartialEq,
{
    /// last known state
    state: HashMap<K, V>,
    /// listeners
    listeners: HashMap<uuid::Uuid, mpsc::Sender<Event<K, V>>>,
}

impl<K, V> Inner<K, V>
where
    K: Clone + Debug + Eq + Hash,
    V: Clone + Debug + PartialEq,
{
    async fn broadcast(mut lock: impl DerefMut<Target = Self>, evt: Event<K, V>) {
        let listeners = stream::iter(&lock.listeners);
        let listeners = listeners.map(|(id, l)| {
            let evt = evt.clone();
            async move {
                if let Err(_) = l.send(evt).await {
                    Some(*id)
                } else {
                    None
                }
            }
        });
        let failed: Vec<uuid::Uuid> = listeners
            .buffer_unordered(10)
            .filter_map(|s| async move { s })
            .collect()
            .await;

        // remove failed subscribers

        for id in failed {
            debug!(?id, "Removing failed listener");
            lock.listeners.remove(&id);
        }
    }
}

impl<K, V> State<K, V>
where
    K: Clone + Debug + Eq + Hash + Send + Sync + 'static,
    V: Clone + Debug + PartialEq + Send + Sync + 'static,
{
    pub async fn subscribe(&self) -> Subscription<K, V> {
        let (tx, rx) = mpsc::channel(16);

        let mut lock = self.inner.write().await;

        // we can "unwrap" here, as we just created the channel and are in control of the two
        // possible error conditions (full, no receiver).
        tx.try_send(Event::Restart(lock.state.clone()))
            .expect("Channel must have enough capacity");

        let id = loop {
            let id = uuid::Uuid::new_v4();
            if let Entry::Vacant(entry) = lock.listeners.entry(id) {
                entry.insert(tx);
                break id;
            }
        };

        let inner = self.inner.clone();

        Subscription::new(rx, move || {
            tokio::spawn(async move {
                inner.write().await.listeners.remove(&id);
            });
        })
    }

    pub async fn get_state(&self) -> HashMap<K, V> {
        self.inner.read().await.state.clone()
    }

    pub async fn set_state(&self, state: HashMap<K, V>) {
        let mut lock = self.inner.write().await;
        lock.state = state.clone();
        Inner::broadcast(lock, Event::Restart(state)).await;
    }

    pub async fn mutate_state<F>(&self, key: K, f: F)
    where
        F: FnOnce(Option<V>) -> Option<V>,
    {
        let mut lock = self.inner.write().await;

        let evt = match lock.state.entry(key.clone()) {
            Entry::Vacant(entry) => {
                if let Some(state) = f(None) {
                    entry.insert(state.clone());
                    Some(Event::Added(key, state))
                } else {
                    None
                }
            }
            Entry::Occupied(mut entry) => match f(Some(entry.get().clone())) {
                Some(state) => {
                    if entry.get() != &state {
                        *entry.get_mut() = state.clone();
                        Some(Event::Modified(key, state))
                    } else {
                        None
                    }
                }
                None => {
                    entry.remove();
                    Some(Event::Removed(key))
                }
            },
        };

        if let Some(evt) = evt {
            Inner::broadcast(lock, evt).await;
        }
    }
}

impl<K, V> Default for State<K, V>
where
    K: Clone + Debug + Eq + Hash,
    V: Clone + Debug + PartialEq,
{
    fn default() -> Self {
        Self {
            inner: Arc::new(RwLock::new(Inner {
                state: Default::default(),
                listeners: Default::default(),
            })),
        }
    }
}
