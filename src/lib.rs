#![crate_name = "hjul"]
#![feature(test)]

extern crate test;

mod timers;

pub use timers::{Runner, Timer};

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    use std::thread;
    use std::time::{Duration, Instant};

    use test::Bencher;

    /* This test can theoretically fail -- on a VERY slow machine.
     */
    #[test]
    fn test_accuracy() {
        let capacity = 100_000;
        let accuracy = Duration::from_millis(100);
        let horrison = Duration::from_millis(10_000);
        let cogs = (horrison.as_nanos() / accuracy.as_nanos()) as usize;
        let runner = Runner::new(accuracy, cogs, capacity);

        let tests = vec![
            Duration::from_millis(100),
            Duration::from_millis(200),
            Duration::from_millis(250),
            Duration::from_millis(200),
            Duration::from_millis(400),
            Duration::from_millis(600),
            Duration::from_millis(700),
            Duration::from_millis(100),
            Duration::from_millis(500),
            Duration::from_millis(500),
        ];

        // create and start the timers
        let mut checks: Vec<Arc<AtomicBool>> = vec![];
        let mut timers: Vec<Timer> = vec![];
        for _ in 0..(capacity / tests.len()) {
            for &dur in &tests {
                let fired = Arc::new(AtomicBool::new(false));
                let fcopy = fired.clone();
                let start = Instant::now();
                let timer = runner.timer(move || {
                    let delta = Instant::now() - start;
                    assert!(
                        delta >= dur - accuracy,
                        "at least (duration - accuracy) time should pass"
                    );
                    assert!(
                        delta + accuracy >= dur,
                        "no more than (duration + accuracy) time should pass"
                    );
                    fcopy.store(true, Ordering::SeqCst);
                });
                timer.reset(dur);
                timers.push(timer);
                checks.push(fired);
            }
        }

        // wait for timers to fire
        let mut longest = tests[0];
        for &dur in &tests {
            if dur > longest {
                longest = dur;
            }
            assert!(dur < horrison);
        }
        thread::sleep(2 * longest);

        // check that every timer fired
        for f in checks {
            assert!(f.load(Ordering::Acquire));
        }
    }

    #[bench]
    fn bench_reset(b: &mut Bencher) {
        let runner = Runner::new(Duration::from_millis(100), 100, 10);
        let timer = runner.timer(Box::new(|| {}));
        b.iter(|| timer.reset(Duration::from_millis(1000)));
    }

    #[bench]
    fn bench_start(b: &mut Bencher) {
        let runner = Runner::new(Duration::from_millis(100), 100, 10);
        let timer = runner.timer(Box::new(|| {}));
        b.iter(|| timer.start(Duration::from_millis(1000)));
    }
}
