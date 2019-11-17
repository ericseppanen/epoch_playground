use crossbeam::epoch::{pin, Atomic};
use std::sync::atomic::Ordering;

#[derive(Debug)]
struct Canary {
    name: String,
}

impl Canary {
    fn new(name: &str) -> Canary {
        Canary {
            name: name.to_owned(),
        }
    }
}

impl Drop for Canary {
    fn drop(&mut self) {
        println!("{}: dropped", self.name);
    }
}

struct BirdCage {
    c: Vec<Atomic<Canary>>,
}

impl BirdCage {
    fn new(size: usize) -> BirdCage {
        let mut bc = BirdCage {
            c: Vec::with_capacity(size),
        };
        for ii in 0..size {
            let name = format!("Canary {}", ii);
            bc.c.push(Atomic::new(Canary::new(&name)));
        }
        bc
    }

    fn access(&self, n: usize, ctx: &str) {
        let guard = &pin();
        let shared = self.c[n].load(Ordering::SeqCst, guard);
        let c: &Canary = unsafe{shared.as_ref()}.unwrap();
        println!("[{}] accessing {}", ctx, c.name);
        let defer_ctx = ctx.to_owned();
        guard.defer(move || {
            cleanup(defer_ctx);
        });

        // Uncomment this to see the deferred function run sooner.
        // Otherwise, the default Collector will wait until a bunch of
        // deferred actions have accumulated (~256 in crossbeam 0.7.3).

        //guard.flush();
    }
}

fn cleanup(ctx: String) {
    println!("[{}] doing deferred cleanup", ctx);
}


fn main() {
    let bc_size = 10;
    let birdcage = BirdCage::new(bc_size);
    // Increase this number to see the deferred functions run.
    for n in 0..bc_size {
        birdcage.access(n, "main");
    }
}
