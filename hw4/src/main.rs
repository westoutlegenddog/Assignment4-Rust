use futures::future::LocalBoxFuture;
use futures::FutureExt;
use lazy_static::lazy_static;
use std::cell::RefCell;
use std::collections::HashSet;
use std::collections::VecDeque;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::Condvar;
use std::sync::Mutex;
use std::sync::MutexGuard;
use std::task::Context;
use std::task::Poll;
use std::task::Wake;
use std::task::Waker;

lazy_static! {
    static ref COUNTERS: Mutex<u32> = Mutex::new(0);
    static ref COUNTERW: Mutex<u32> = Mutex::new(0);
}

struct Demo;

impl Future for Demo {
    type Output = ();
    fn poll(
        self: std::pin::Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Self::Output> {
        println!("hello world!");
        std::task::Poll::Ready(())
    }
}

struct Signal {
    state: Mutex<State>,
    cond: Condvar,
}

enum State {
    Empty,
    Waiting,
    Notified,
}

impl State {
    fn new() -> Self {
        State::Empty
    }
}

impl Signal {
    fn new() -> Self {
        let state = Mutex::new(State::new()); //
        let cond = Condvar::new();
        Signal { state, cond }
    }

    fn wait(&self) {
        let mut state: MutexGuard<'_, State> = self.state.lock().unwrap();
        match *state {
            State::Notified => *state = State::Empty,
            State::Waiting => {
                panic!("multiple wait");
            }
            State::Empty => {
                *state = State::Waiting;
                while let State::Waiting = *state {
                    state = self.cond.wait(state).unwrap();
                }
            }
        }
    }

    fn notify(&self) {
        let mut state: MutexGuard<'_, State> = self.state.lock().unwrap();
        match *state {
            State::Notified => {}
            State::Empty => *state = State::Notified,
            State::Waiting => {
                *state = State::Empty;
                self.cond.notify_one();
            }
        }
    }
}

impl Wake for Signal {
    fn wake(self: Arc<Self>) {
        self.notify();
    }
}

struct Task {
    future: RefCell<LocalBoxFuture<'static, ()>>,
    signal: Arc<Signal>,
}
unsafe impl Send for Task {}
unsafe impl Sync for Task {}

/////////////////////////////////////////
impl Wake for Task {
    fn wake(self: Arc<Self>) {
        /*
        RUNNABLE.with(|runnable| runnable.lock().unwrap().push_back(self.clone()));
        self.signal.notify();
        */
        let mut counterw = COUNTERW.lock().unwrap();
        *counterw += 1;
        if *counterw % 2 == 0 {
            RUNNABLE.with(|runnable| runnable.queue1.lock().unwrap().push_back(self.clone()));
        } else {
            RUNNABLE.with(|runnable| runnable.queue2.lock().unwrap().push_back(self.clone()));
        }
        self.signal.notify();
    }
}

scoped_tls::scoped_thread_local!(static SIGNAL: Arc<Signal>);
scoped_tls::scoped_thread_local!(static RUNNABLE: Arc<Queues>);

struct Queues {
    queue1: Mutex<VecDeque<Arc<Task>>>,
    queue2: Mutex<VecDeque<Arc<Task>>>,
}

fn block_on<F: Future>(future: F) -> F::Output {
    let mut thread_ids = HashSet::new();
    let current_thread_id = thread_id::get();
    thread_ids.insert(current_thread_id);

    let mut main_fut: Pin<&mut F> = std::pin::pin!(future);
    let signal: Arc<Signal> = Arc::new(Signal::new());
    let waker: Waker = Waker::from(signal.clone());
    let mut cx: Context<'_> = Context::from_waker(&waker);
    //let runnable = Mutex::new(VecDeque::with_capacity(1024));
    let runnable: Arc<Queues> = Arc::new(Queues {
        queue1: Mutex::new(VecDeque::new()),
        queue2: Mutex::new(VecDeque::new()),
    });
    SIGNAL.set(&signal, || {
        RUNNABLE.set(&runnable, || loop {
            if let Poll::Ready(output) = main_fut.as_mut().poll(&mut cx) {
                return output;
            }
            let runnable1 = runnable.clone();
            /*
            while let Some(task) = runnable.lock().unwrap().pop_front() {
                let waker: Waker = Waker::from(task.clone());
                let mut cx: Context<'_> = Context::from_waker(&waker);
                let _ = task.future.borrow_mut().as_mut().poll(&mut cx);
            }
            signal.wait();
            */
            let new_thread = std::thread::spawn(move || {
                while let Some(task) = runnable1.queue2.lock().unwrap().pop_front() {
                    let waker = Waker::from(task.clone());
                    let mut cx = Context::from_waker(&waker);
                    let _poll_result = task.future.borrow_mut().as_mut().poll(&mut cx);
                }
            });
            while let Some(task) = runnable.queue1.lock().unwrap().pop_front() {
                let waker = Waker::from(task.clone());
                let mut cx = Context::from_waker(&waker);
                let _poll_result = task.future.borrow_mut().as_mut().poll(&mut cx);
            }
            new_thread.join().unwrap();
            signal.wait();
        })
    })
}

fn spawn<F: Future<Output = ()> + 'static>(fut: F) {
    let task = Arc::new(Task {
        future: RefCell::new(fut.boxed_local()),
        signal: Arc::new(Signal::new()),
    });
    let mut counters = COUNTERS.lock().unwrap();
    *counters += 1;
    if *counters % 2 == 0 {
        RUNNABLE.with(|runnable| runnable.queue1.lock().unwrap().push_back(task));
    } else {
        RUNNABLE.with(|runnable| runnable.queue2.lock().unwrap().push_back(task));
    }
}

/*
async fn demo() {
    let (tx, rx) = async_channel::bounded(1);
    spawn(demo2(tx));
    println!("hello world!");
    let _ = rx.recv().await;
}

async fn demo2(tx: async_channel::Sender<()>) {
    println!("hello world2!");
    let _ = tx.send(()).await;
}

fn main() {
    block_on(demo());
}
*/

fn cpu() -> usize {
    let mut sum = 0;
    for _i in 0..1000 {
        for _j in 0..1000 {
            sum += 1;
        }
    }
    sum
}

async fn demo1(tx: async_channel::Sender<()>) {
    let result = cpu();
    println!("demo1, 1000000次累加结果为{}", result);
    println!("demo1 will sleep for 5s");
    std::thread::sleep(std::time::Duration::from_secs(5));
    println!("demo1 wakes up");
    println!("demo1 的 thread id 为 {}", thread_id::get());
    let _ = tx.send(()).await;
}

async fn demo2(tx: async_channel::Sender<()>) {
    let result = cpu();
    println!("demo2, 1000000次累加结果为{}", result);
    println!("demo2 will sleep for 5s");
    std::thread::sleep(std::time::Duration::from_secs(5));
    println!("demo2 wakes up");
    println!("demo2 的 thread id 为 {}", thread_id::get());
    let _ = tx.send(()).await;
}

async fn demo() {
    let (tx1, rx1) = async_channel::bounded::<()>(1);
    let (tx2, rx2) = async_channel::bounded::<()>(1);
    spawn(demo1(tx1));
    spawn(demo2(tx2));
    let _ = rx1.recv().await;
    let _ = rx2.recv().await;
}

fn main() {
    println!("多任务多线程测试：");
    block_on(demo());
}
