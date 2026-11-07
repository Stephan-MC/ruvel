use ruvel::pipeline::{BoxFuture, Next, Pipe, Pipeline};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

struct AddPrefixPipe;

impl Pipe<String> for AddPrefixPipe {
    fn handle<'a>(&'a self, passable: String, next: Next<String, String>) -> BoxFuture<'a, String> {
        Box::pin(async move {
            let res = next.run(format!("prefix_{}", passable)).await;
            format!("{}_suffix", res)
        })
    }
}

struct StopEarlyPipe;

impl Pipe<String> for StopEarlyPipe {
    fn handle<'a>(&'a self, _passable: String, _next: Next<String, String>) -> BoxFuture<'a, String> {
        Box::pin(async move { "stopped_early".to_string() })
    }
}

struct TrackingPipe {
    id: usize,
    log: Arc<std::sync::Mutex<Vec<String>>>,
}

impl Pipe<String> for TrackingPipe {
    fn handle<'a>(&'a self, passable: String, next: Next<String, String>) -> BoxFuture<'a, String> {
        Box::pin(async move {
            self.log
                .lock()
                .unwrap()
                .push(format!("before_{}", self.id));
            let result = next.run(passable).await;
            self.log
                .lock()
                .unwrap()
                .push(format!("after_{}", self.id));
            result
        })
    }
}

#[tokio::test]
async fn test_pipeline_with_struct_pipes_and_destination() {
    let result = Pipeline::new()
        .send("input".to_string())
        .pipe(AddPrefixPipe)
        .then(|passable| async move { format!("{}_handled", passable) })
        .await;

    assert_eq!(result, "prefix_input_handled_suffix");
}

#[tokio::test]
async fn test_pipeline_with_then_return() {
    let result = Pipeline::new()
        .send("hello".to_string())
        .pipe(AddPrefixPipe)
        .then_return()
        .await;

    assert_eq!(result, "prefix_hello_suffix");
}

#[tokio::test]
async fn test_pipeline_onion_order() {
    let log = Arc::new(std::sync::Mutex::new(Vec::new()));

    let pipe1 = TrackingPipe {
        id: 1,
        log: Arc::clone(&log),
    };
    let pipe2 = TrackingPipe {
        id: 2,
        log: Arc::clone(&log),
    };

    let result = Pipeline::new()
        .send("core".to_string())
        .through(vec![pipe1, pipe2])
        .then(|val| async move { format!("{}_executed", val) })
        .await;

    assert_eq!(result, "core_executed");

    let entries = log.lock().unwrap().clone();
    assert_eq!(entries, vec!["before_1", "before_2", "after_2", "after_1"]);
}

#[tokio::test]
async fn test_pipeline_short_circuit() {
    let called = Arc::new(AtomicUsize::new(0));
    let called_clone = Arc::clone(&called);

    let result = Pipeline::new()
        .send("start".to_string())
        .pipe(StopEarlyPipe)
        .then(move |_val| {
            let called = Arc::clone(&called_clone);
            async move {
                called.fetch_add(1, Ordering::SeqCst);
                "should_not_reach".to_string()
            }
        })
        .await;

    assert_eq!(result, "stopped_early");
    assert_eq!(called.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn test_pipeline_closure_pipe() {
    let result = Pipeline::new()
        .send(10)
        .pipe(|val: i32, next: Next<i32, i32>| -> BoxFuture<'static, i32> {
            Box::pin(async move {
                let res = next.run(val * 2).await;
                res + 1
            })
        })
        .pipe(|val: i32, next: Next<i32, i32>| -> BoxFuture<'static, i32> {
            Box::pin(async move {
                let res = next.run(val + 5).await;
                res * 3
            })
        })
        .then(|val| async move { val })
        .await;

    // val = 10
    // pipe 1 sends val * 2 = 20
    // pipe 2 sends val + 5 = 25
    // destination returns 25
    // pipe 2 unwinds: 25 * 3 = 75
    // pipe 1 unwinds: 75 + 1 = 76
    assert_eq!(result, 76);
}

#[tokio::test]
async fn test_pipeline_empty_pipes() {
    let result = Pipeline::new()
        .send(42)
        .then(|val| async move { val * 2 })
        .await;

    assert_eq!(result, 84);

    let returned = Pipeline::new().send(100).then_return().await;
    assert_eq!(returned, 100);
}

#[tokio::test]
async fn test_pipeline_different_input_and_output_types() {
    struct RequestWrapper(String);
    struct ResponseWrapper(usize);

    let result = Pipeline::<RequestWrapper, ResponseWrapper>::new()
        .send(RequestWrapper("hello world".to_string()))
        .pipe(
            |req: RequestWrapper,
             next: Next<RequestWrapper, ResponseWrapper>|
             -> BoxFuture<'static, ResponseWrapper> {
                Box::pin(async move {
                    let mut res = next.run(req).await;
                    res.0 += 10;
                    res
                })
            },
        )
        .then(|req| async move { ResponseWrapper(req.0.len()) })
        .await;

    // "hello world".len() = 11, + 10 = 21
    assert_eq!(result.0, 21);
}
