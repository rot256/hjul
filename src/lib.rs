use mio::{Events, Poll, PollOpt, Ready, Token};
use mio_extras::timer;

use spin::Mutex;
use std::mem;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Weak};
use std::thread;
use std::time::Duration;

const TIMER: Token = Token(2);

type Callback = Box<dyn Fn() -> () + Sync + Send + 'static>;
type State = Weak<TimerInner>;

struct TimerInner {
    pending: AtomicBool,
    runner: Arc<RunnerInner>,
    timeout: Mutex<Option<timer::Timeout>>,
    callback: Callback,
}

type RunnerInner = Mutex<timer::Timer<State>>;

pub struct Timer(Arc<TimerInner>);
pub struct Runner(Arc<RunnerInner>);

impl Runner {
    pub fn new(tick: Duration, slots: usize) -> Runner {
        // create timer whell
        let builder: timer::Builder = Default::default();
        let builder = timer::Builder::tick_duration(builder, tick);
        let builder = timer::Builder::num_slots(builder, slots);
        let timer = timer::Builder::build(builder);

        // create Poll
        let poll = Poll::new().unwrap();
        poll.register(&timer, TIMER, Ready::readable(), PollOpt::edge())
            .unwrap();

        // allow timer to be shared by threads
        let inner = Arc::new(Mutex::new(timer));

        // start callback thread
        {
            let timer = inner.clone();
            let mut events = Events::with_capacity(1024);
            thread::spawn(move || loop {
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
                        _ => println!("awkward"),
                    }
                }
            });
        }

        // return runner handle
        Runner(inner)
    }

    pub fn timer(self, callback: Callback) -> Timer {
        Timer(Arc::new(TimerInner {
            callback,
            pending: AtomicBool::new(false),
            runner: self.0.clone(),
            timeout: Mutex::new(None),
        }))
    }
}

impl Timer {
    fn stop(&self) {
        let inner = &self.0;

        if inner.pending.swap(false, Ordering::Acquire) {
            if let Some(tm) = mem::replace(&mut *inner.timeout.lock(), None) {
                inner.runner.lock().cancel_timeout(&tm);
            }
        }
    }

    fn reset(&self, duration: Duration) {
        let inner = &self.0;

        inner.pending.store(true, Ordering::SeqCst);
        let mut timeout = inner.timeout.lock();
        let mut timer = inner.runner.lock();
        let new = timer.set_timeout(duration, Arc::downgrade(&self.0));
        if let Some(tm) = mem::replace(&mut *timeout, Some(new)) {
            timer.cancel_timeout(&tm);
        }
    }

    fn start(&self, duration: Duration) {
        let inner = &self.0;

        if !inner.pending.swap(true, Ordering::SeqCst) {
            let mut timeout = inner.timeout.lock();
            if timeout.is_some() {
                return;
            }
            let mut timer = inner.runner.lock();
            *timeout = Some(timer.set_timeout(duration, Arc::downgrade(&self.0)));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn it_works() {
        let runner = Runner::new(Duration::from_millis(100), 10 * 60 * 4);

        let t = Arc::new(Mutex::new(5));
        let timer = {
            let t = t.clone();
            runner.timer(Box::new(move || {
                *t.lock() = 5;
                println!("run")
            }))
        };

        timer.reset(Duration::from_millis(200));
        timer.stop();

        thread::sleep(Duration::from_millis(10_000));
    }
}
