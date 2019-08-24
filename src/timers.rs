use mio::{Events, Poll, PollOpt, Ready, Token};
use mio_extras::timer;

use spin::Mutex;

use std::mem;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Weak};
use std::thread;
use std::time::Duration;

const TIMER: Token = Token(1);

type Callback = Box<dyn Fn() -> () + Sync + Send + 'static>;
type State = Weak<TimerInner>;

struct TimerInner {
    timer: Arc<Mutex<timer::Timer<State>>>,
    pending: AtomicBool,
    timeout: Mutex<Option<timer::Timeout>>,
    callback: Callback,
}

pub struct Runner {
    timer: Arc<Mutex<timer::Timer<State>>>,
    handle: Option<thread::JoinHandle<()>>,
    running: Arc<AtomicBool>,
}

#[derive(Clone)]
pub struct Timer(Arc<TimerInner>);

impl Runner {
    pub fn new(tick: Duration, slots: usize, capacity: usize) -> Runner {
        // create timer whell
        let builder: timer::Builder = Default::default();
        let builder = timer::Builder::tick_duration(builder, tick);
        let builder = timer::Builder::num_slots(builder, slots);
        let builder = timer::Builder::capacity(builder, capacity);
        let timer = timer::Builder::build(builder);

        // create Poll
        let poll = Poll::new().unwrap();
        poll.register(&timer, TIMER, Ready::readable(), PollOpt::level())
            .unwrap();

        // allow sharing state
        let timer = Arc::new(Mutex::new(timer));
        let running = Arc::new(AtomicBool::new(true));

        // start callback thread
        let handle = {
            let timer = timer.clone();
            let running = running.clone();
            thread::spawn(move || {
                let mut events = Events::with_capacity(256);
                while running.load(Ordering::Acquire) {
                    poll.poll(&mut events, None).unwrap();
                    for event in &events {
                        match event.token() {
                            TIMER => {
                                if let Some(weak) = timer.lock().poll() {
                                    let weak: Weak<TimerInner> = weak;
                                    match weak.upgrade() {
                                        Some(timer) => {
                                            if timer.pending.swap(false, Ordering::SeqCst) {
                                                (timer.callback)()
                                            }
                                        }
                                        None => (),
                                    }
                                }
                            }
                            _ => unreachable!(),
                        }
                    }
                }
            })
        };

        // return runner handle
        Runner {
            timer,
            handle: Some(handle),
            running,
        }
    }

    pub fn timer(&self, callback: Callback) -> Timer {
        Timer(Arc::new(TimerInner {
            callback,
            pending: AtomicBool::new(false),
            timer: self.timer.clone(),
            timeout: Mutex::new(None),
        }))
    }
}

impl Drop for Runner {
    fn drop(&mut self) {
        // mark the runner as stopped
        self.running.store(false, Ordering::SeqCst);

        // create an event for mio
        self.timer
            .lock()
            .set_timeout(Duration::from_millis(0), Weak::new());

        // join with the callback thread
        if let Some(handle) = mem::replace(&mut self.handle, None) {
            handle.join().unwrap();
        }
    }
}

impl TimerInner {
    fn stop(&self) {
        if self.pending.swap(false, Ordering::Acquire) {
            if let Some(tm) = mem::replace(&mut *self.timeout.lock(), None) {
                self.timer.lock().cancel_timeout(&tm);
            }
        }
    }
}

impl Timer {
    pub fn stop(&self) {
        self.0.stop()
    }

    pub fn reset(&self, duration: Duration) {
        let inner = &self.0;

        inner.pending.store(true, Ordering::SeqCst);
        let mut timeout = inner.timeout.lock();
        let mut timer = inner.timer.lock();
        let new = timer.set_timeout(duration, Arc::downgrade(&self.0));
        if let Some(tm) = mem::replace(&mut *timeout, Some(new)) {
            timer.cancel_timeout(&tm);
        }
    }

    pub fn start(&self, duration: Duration) {
        // optimistic check for pending
        let inner = &self.0;
        if inner.pending.load(Ordering::Acquire) {
            return;
        }

        // take lock and set if not pending
        let mut timeout = inner.timeout.lock();
        if timeout.is_some() {
            return;
        }
        let mut timer = inner.timer.lock();
        *timeout = Some(timer.set_timeout(duration, Arc::downgrade(&self.0)));
        inner.pending.store(true, Ordering::SeqCst);
    }
}

impl Drop for TimerInner {
    fn drop(&mut self) {
        self.stop()
    }
}
