**This is a copy of a blog post originally published [here](https://codeandbitters.com/learning-rust-crossbeam-epoch/).**

I'm posting a copy on github to make it easier for others to point out where I've made mistakes.

---

I've been experimenting with Rust lately, and have just started following Jon Gjengset's videos.  In his [latest livestream](https://www.youtube.com/watch?v=yQFWmGaFBjk), Jon started a port of the Java `ConcurrentHashMap` to Rust.  I highly recommend them; it's fun to see how someone with a lot of Rust experience approaches problems.

One of the things that grabbed my attention was his use of [`crossbeam::epoch`](https://docs.rs/crossbeam/latest/crossbeam/epoch/index.html).  It's a library that helps you write certain kinds of lock-free concurrent data structures, using an "epoch counter" that allows you to disconnect objects from your main data structure and they will be freed later-- without using any per-object atomic reference counters or other major GC overhead.

For some more background, read [this post](https://aturon.github.io/blog/2015/08/27/epoch/) by Aaron Turon, who wrote the first version of `crossbeam`, as well as [this paper by Keir Fraser](https://www.cl.cam.ac.uk/techreports/UCAM-CL-TR-579.pdf), which goes into greater detail.

How is epoch-based reclamation done in Rust?  That's what I wanted to know.

I'll describe my experience with learning the basics of `crossbeam::epoch`, and show you the code that I wrote along the way.  I'll start with some simple experiments, and then move on to a simple lock-free data structure.

If you want to see my actual code, you can find it here:
[epoch_playground](https://github.com/ericseppanen/epoch_playground)

Before I dive in, a few more notes:
- This probably isn't a good place to start if you've never written _any_ rust code.
- This is an example of doing fairly extreme things in search of the best performance.  If you don't need performance, just use a plain `Mutex` or `Arc`   unless you like doing things the hard way.
- This is a demonstration of how to build a low-level data structure, not an application.  I wouldn't expect to see `crossbeam::epoch` invoked directly by application code.  If you just want to _use_ concurrent data structures, my experience here might not be helpful.

Also, thanks to Jon Gjengset for making advanced Rust videos.  They've helped me discover some really interesting parts of the Rust universe.

Without further delay, let's begin!

## Part 1: Hello, Canary

I began by creating a skeleton project:
```
$ cargo init . --name epoch_playground --bin
```

and adding this to `Cargo.toml`:
```
[dependencies]
crossbeam = "0.7"
```

Since we're going to be playing around with delayed object destruction, let's create an object that announces its destruction to stdout:

```
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

fn main() {
    let _a = Canary::new("first");
}
```

If I run this code, I get:
```
first: dropped
```
... which is pretty much what one would expect.


## Part 2: First try at `crossbeam::epoch`

Let's bring in the `crossbeam` module and use it to access a `Canary` object.

```
use crossbeam::epoch::{pin, Atomic};
use std::sync::atomic::Ordering;

// ...

fn main() {
    let a = Atomic::new(Canary::new("first"));

    let guard = &pin();
    let shared = a.load(Ordering::SeqCst, guard);
    let c: &Canary = unsafe{shared.as_ref()}.unwrap();
    println!("accessing {}", c.name);
}
```

We're storing the `Canary` in an [`Atomic`](https://docs.rs/crossbeam/0.7.3/crossbeam/epoch/struct.Atomic.html).  What does this mean exactly?

We're creating a `Canary` on the heap.  The `Atomic` stores a pointer to the `Canary`.  Yes, we're still writing Rust code, so "pointer" should make you pause for a moment.  We're going to be writing some really low-level code here, and the semantics are basically those of raw pointers.  `crossbeam` will help us manage their lifetimes, but a lot of the correctness burden is still on us.

The `guard` and `pin()` seem a little mysterious.  The guard is a [`Guard`](https://docs.rs/crossbeam/0.7.3/crossbeam/epoch/struct.Guard.html), and it will allow us to access things that are managed by the epoch system.  `pin()` is just a function that creates a `Guard` for us; we are "pinning" the current thread (which just means that we are accessing some epoch-managed objects).

The `Guard` isn't doing much yet-- it will allow us to track the proper lifetime of objects, but for now it's just boilerplate that allows us to access the thing in the `Atomic`.

The type of our `shared` variable is [`Shared`](https://docs.rs/crossbeam/0.7.3/crossbeam/epoch/struct.Shared.html).  A `Shared<T>` is a specialized ref to the inner `T`, but it allows rust to do some magic lifetime tracking: we want to prove that our access to the `T` doesn't outlast the `Guard`.

[`std::sync::atomic::Ordering`](https://doc.rust-lang.org/std/sync/atomic/enum.Ordering.html) is a big topic, and maybe I'll cover that later.  Consider it a placeholder for something I'll need to think about once I know how that value will be accessed.

(quick note: I've decided to make links to the crossbeam docs point at version 0.7.3 so it's clear what I was seeing when I was writing this.)

Note that the `Atomic` doesn't behave like a rust `Box` or other container, in that it doesn't manage the lifetime of the `Canary`.  If I run the code, I no longer see the `Canary`'s "dropped" message.  So we're leaking memory now.  Hopefully we'll learn how to deal with this soon.

Should I have said something about the use of `unsafe`?  It's kind of unavoidable; if we want to design fast lock-free data structures in Rust we'll need to come to terms with the fact that memory safety is our problem again.  Even though I'm doing my unsafe hacking in `main()` right now, that's not how we'd do it if we were building this for use by others: we'd hide the `unsafe` bits in a crate where nobody but us needs to worry about it.


## Part 3: Deferred work

One of the critical parts about epoch-based memory management is the concept of "deferred" work.  The thing we want to defer is deleting objects after they've been ejected from our data structure, but crossbeam will let you call arbitrary functions.

`crossbeam::epoch` lets us do this with calls to the `Guard` object, so let's play around with `Guard::defer()`.

I want to start pulling the interesting code out of `main` and start thinking about my creation as a proper data structure, so we need a higher level container.  Since we called our individual object `Canary`, I called this struct `BirdCage`.

```
struct BirdCage {
    c: Atomic<Canary>,
}
```

Let's add multiple accesses to the `BirdCage`, and create some deferred work on each access.

```
fn cleanup(n: usize) {
    println!("[{}] doing deferred cleanup", n);
}

fn access(birdcage: &BirdCage, n: usize) {
    let guard = &pin();
    let shared = birdcage.c.load(Ordering::SeqCst, guard);
    let c: &Canary = unsafe{shared.as_ref()}.unwrap();
    println!("[{}] accessing {}", n, c.name);
    guard.defer(move || {
        cleanup(n);
    });
}

fn main() {
    let birdcage = BirdCage {
        c: Atomic::new(Canary::new("first")),
    };
    // Increase this number to see the deferred functions run.
    for n in 0..10 {
        access(&birdcage, n);
    }
}
```

If we run the code now, there's a surprise: our deferred work never runs-- the program simply ends.

It looks like the default crossbeam behavior is to accumulate a lot of work on the local thread before pushing that work to the global `Collector`, which will somehow trigger the actual running of the deferred work.  There's a `Guard::flush()` call to do that pushing immediately, if you're impatient.  Alternatively, if we do a lot more loop iterations (say, 1000) you will see that most of the deferred work does run.

One final batch of deferred work will never run.  I'm guessing that this is by design (who cares if we failed to free a bunch of data right before the process exits), but maybe we'll figure that out later.


## Part 4: A bigger birdcage

To really exercise deferred memory reclamation, we need a bigger data structure.  So let's make our `BirdCage` hold an arbitrary number of `Atomic<Canary>` elements.  Also add a `new` constructor and move the `access` function into the `BirdCage` itself.

```
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
    }
}

fn cleanup(ctx: String) {
    println!("[{}] doing deferred cleanup", ctx);
}
```

Note the change in function arguments from the previous code.  `access()` was about to get confusing because I used an integer for the "context" string, so instead I'm switching that to a string so I can have an integer argument specifying which element to access.

I had to fiddle with the `cleanup()` signature a bit, because the borrow checker got cranky.  Because we can't know when `cleanup()` will actually run, any arguments to it must be passed by value or have `'static` lifetime.  I chose to just copy the context string to keep things simple.

Our `main` function will still be pretty boring: just run in a loop accessing each of the individual elements of our `BirdCage`.

```
fn main() {
    let bc_size = 10;
    let birdcage = BirdCage::new(bc_size);
    // Increase this number to see the deferred functions run.
    for n in 0..bc_size {
        birdcage.access(n, "main");
    }
}
```


## Part 5: Creating garbage


If we're going to do concurrent operations requiring memory management, we're going to need to start remove things from our data structure (`BirdCage`).

Let's add a `replace()` method, which goes into our data structure and removes one `Canary` and replaces it with another.  We use `Atomic::swap()` to do this.

```
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

    let defer_ctx = ctx.to_owned();
    guard.defer(move || {
        cleanup(defer_ctx);
    });
}
```

We moved the call to `defer()` to the `replace()` function, because eventually we're going to make it collect the newly created garbage.  But we're not quite there yet.

Note we have removed the `Canary` pointer from the main data structure, but we haven't actually done anything to free it.  Each time `replace()` runs, it leaks one `Canary`.


## Part 6: Deferred destruction

When our data structure releases an item, we want it to be automatically destroyed once the global epoch has advanced.  This could be used by calling `defer()` and handing over the value to be destroyed, but that's hard to do because you'd have to extract the pointed from the `Shared` (which can't outlive the `Guard`).

Conveniently, there is a `defer_destroy()` function that does exactly what we want.

```
fn replace(&self, n: usize, ctx: &str, new_c: Canary) {
    // ...

    // Now schedule the stolen canary for deallocation.
    // This is equivalent to defer() with a closure that drops the value.
    unsafe {
        guard.defer_destroy(stolen_c);
    }
}
```

As in previous examples, you need to run through a lot of `pin()`s and accumulated deferred work before any of it actually happens; play with the numbers (and try the `flush()` call) to see different effects.

I also got tired of the deferred work not getting to run before the process completes, so I also added this to `main()`:

```
    pin().flush();
    pin().flush();
```

This seems pretty hacky.  To force any deferred work to run, we need the epoch to move forward two times.

I don't think I understand the global epoch counter design well enough to really explain why two calls is necessary, but it seems to me that this sort of internal knowledge shouldn't really be required to do something this basic.  Maybe there's a better way to do this, but I haven't found it yet.

I wish there was a way to say "destroy all the remaining garbage from _this_ data structure," but the epoch counter, `Collector`, and deferred work are global, not per data structure.


## Part 6: Randomized multithreaded workload

Now that we have what looks like a basic concurrent data structure, we should  add some threads to do random unpredictable things to it.  We'd like to have callers to `access()` and `replace()` bumping into each other constantly.

First: spawn a bunch of threads.  We need to be a little careful with passing things to threads, since Rust can't reason about whether they might outlive the main thread.  So we'll put the `BirdCage` in an `Arc`, which will allow us to make thread-safe references.

We'll also number the threads so we can print out interesting messages from each.

Finally, collect all the thread handles and `join()` them so `main()` doesn't exit prematurely.

```
use std::sync::Arc;
use std::thread;
use rand::Rng;

// ...

const ITERATIONS: usize = 100;
const BIRDCAGE_SIZE: usize = 10;
const NUM_THREADS: usize = 10;

fn worker(birdcage: &BirdCage, id: usize) {
    let bc_size = birdcage.c.len();
    let my_name = format!("thread {}", id);
    let mut rng = rand::thread_rng();

    for n in 0..ITERATIONS {
        // read-only access of a random element
        let pick1 = rng.gen_range(0, bc_size);
        birdcage.access(pick1, &my_name);

        // replace a random element with a new one.
        let c = Canary::new(&format!("{} Cuckoo {}", my_name, n));
        let pick2 = rng.gen_range(0, bc_size);
        birdcage.replace(pick2, &my_name, c);
    }
    println!("{} exiting", my_name);
}

fn main() {
    // Increase this number to see how much deferred work gets buffered.
    let birdcage = Arc::new(BirdCage::new(BIRDCAGE_SIZE));
    let mut thread_handles = Vec::new();

    for thread_id in 0..NUM_THREADS {
        let local_id = thread_id;
        let local_birdcage = birdcage.clone();
        let handle = thread::spawn(move ||
            worker(local_birdcage.as_ref(), local_id)
        );
        thread_handles.push(handle);
    }

    for handle in thread_handles {
        handle.join().unwrap();
    }
}
```

We'd like the threads to bang around randomly (hopefully triggering race conditions of various kinds), so we'll also randomize which locations each thread accesses.  Each thread will loop for a while, doing repetitions of `access(random_index)` and then `replace(another_random_index)`.

It's useful to crank up the number of threads, or iterations, or both, and watch what happens. Increasing the birdcage size will actually reduce contention, so try making that smaller.  This won't prove that our data structure is correct, but it's nice to see that it will run as long as you want without crashing.

It's also worth checking the speed of debug builds vs release.  You probably want to remove all the `println!` statements because the different threads are spending most of their time fighting over the `stdout` mutex and not doing real work.  The release build winds up being about 10x faster on my machine.

I think that's enough for this first post.  I know I left a few open questions and never got around to discussing the atomic ordering semantics at all, but I'll postpone those for a future post.

If I got anything wrong and you'd like to see it fixed, feel free to raise an issue on the [github repo](https://github.com/ericseppanen/epoch_playground).
