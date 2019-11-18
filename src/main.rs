use crossbeam::epoch::{pin, Atomic, Owned};
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
    }

    fn replace(&self, n: usize, ctx: &str, new_c: Canary) {
        println!("[{}] put {} into slot {}", ctx, new_c.name, n);

        let guard = &pin();

        // swap() will only accept a Shared or Owned, so let's make one of those.
        // There are multiple ways to write this code but Owned seems to signal
        // my intent (because at this point I'm the sole owner.)
        let owned_new_c = Owned::new(new_c);

        // We are stealing whatever Canary happens to be present in this
        // location, and substituting a new one.
        let stolen_c = self.c[n].swap(owned_new_c, Ordering::SeqCst, guard);
        let c: &Canary = unsafe{stolen_c.as_ref()}.unwrap();
        println!("[{}] removed {}", ctx, c.name);

        // Now schedule the stolen canary for deallocation.
        // This is equivalent to defer() with a closure that drops the value.
        unsafe {
            guard.defer_destroy(stolen_c);
        }

        // Uncomment this to see the deferred function run sooner.
        // Otherwise, the default Collector will wait until a bunch of
        // deferred actions have accumulated (~256 in crossbeam 0.7.3).

        //guard.flush();
    }


}



fn main() {
    // Increase this number to see how much deferred work gets buffered.
    let bc_size = 10;
    let birdcage = BirdCage::new(bc_size);
    for n in 0..bc_size {
        birdcage.access(n, "main");
    }
    for n in 0..bc_size {
        let c = Canary::new(&format!("Cuckoo {}", n));
        birdcage.replace(n, "main", c);
    }

    // This seems pretty hacky.  To force any deferred work to run, we need the epoch
    // to move forward two times.  The magic number two is due to the inner workings
    // of the global epoch counter.
    // I wish there was a way to say "destroy all the remaining garbage from _this_
    // data structure," but the epoch counter, Collector, and deferred work are
    // global, not per data structure.
    pin().flush();
    pin().flush();
}
