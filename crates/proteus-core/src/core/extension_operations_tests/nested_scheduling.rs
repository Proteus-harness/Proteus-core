use super::*;
use std::time::Duration;
use tokio::{
    sync::{Semaphore, mpsc},
    task::JoinHandle,
    time::timeout,
};

const GUARD: Duration = Duration::from_secs(3);

struct CaptureParent {
    captured: mpsc::UnboundedSender<Arc<dyn ToolHost>>,
    release: Arc<Semaphore>,
}

#[async_trait]
impl Tool for CaptureParent {
    fn spec(&self) -> ToolSpec {
        spec("parent", false)
    }

    async fn invoke(&self, call: &ToolCall, ctx: ToolContext) -> Result<ToolResult> {
        self.captured.send(ctx.host.expect("bound parent host"))?;
        self.release.acquire().await?.forget();
        Ok(ToolResult::ok(call.id.clone(), "parent finished"))
    }
}

fn spec(name: &str, parallel: bool) -> ToolSpec {
    ToolSpec::new(
        name,
        "nested scheduling probe",
        json!({"type":"object","additionalProperties":false}),
        ToolSafety::ReadOnly,
    )
    .with_parallel_tool_calls(parallel)
}

fn call(name: &str) -> ToolCall {
    ToolCall::new(new_call_id(), name, json!({}))
}

struct RunningParent {
    host: Arc<dyn ToolHost>,
    cancellation: CancellationToken,
    release: Arc<Semaphore>,
    task: JoinHandle<Result<ToolResult>>,
}

impl RunningParent {
    async fn start(children: Vec<Arc<dyn Tool>>) -> Self {
        let (captured, mut receiver) = mpsc::unbounded_channel();
        let release = Arc::new(Semaphore::new(0));
        let mut registry = ToolRegistry::new();
        for tool in children.into_iter().chain([Arc::new(CaptureParent {
            captured,
            release: release.clone(),
        }) as Arc<dyn Tool>])
        {
            registry
                .register_arc(ToolSource::builtin("scheduling-test"), tool)
                .unwrap();
        }
        let cancellation = CancellationToken::new();
        let tools = BoundTools::new(
            registry,
            Arc::new(AskChild),
            Arc::new(Approval {
                calls: AtomicUsize::new(0),
                approved: true,
            }),
            Arc::default(),
            ToolExecutionBinding::detached(ExecutionScope::fresh(cancellation.clone())),
        );
        let task = tokio::spawn(async move { tools.execute("/tmp".into(), call("parent")).await });
        let host = timeout(GUARD, receiver.recv())
            .await
            .expect("parent starts")
            .expect("parent exposes its invocation-bound host");
        Self {
            host,
            cancellation,
            release,
            task,
        }
    }

    async fn finish(self) -> ToolResult {
        self.release.add_permits(1);
        timeout(GUARD, self.task)
            .await
            .expect("parent settles")
            .expect("parent task")
            .expect("parent execution")
    }
}

#[derive(Default)]
struct Activity {
    active: AtomicUsize,
    maximum: AtomicUsize,
    events: Mutex<Vec<String>>,
}

struct Probe {
    name: &'static str,
    parallel: bool,
    activity: Arc<Activity>,
    entered: mpsc::UnboundedSender<&'static str>,
    release: Arc<Semaphore>,
}

#[async_trait]
impl Tool for Probe {
    fn spec(&self) -> ToolSpec {
        spec(self.name, self.parallel)
    }

    async fn invoke(&self, call: &ToolCall, _: ToolContext) -> Result<ToolResult> {
        let active = self.activity.active.fetch_add(1, Ordering::SeqCst) + 1;
        self.activity.maximum.fetch_max(active, Ordering::SeqCst);
        self.activity
            .events
            .lock()
            .unwrap()
            .push(format!("{}:start", self.name));
        self.entered.send(self.name)?;
        self.release.acquire().await?.forget();
        self.activity
            .events
            .lock()
            .unwrap()
            .push(format!("{}:finish", self.name));
        self.activity.active.fetch_sub(1, Ordering::SeqCst);
        Ok(ToolResult::ok(call.id.clone(), self.name))
    }
}

async fn entered(receiver: &mut mpsc::UnboundedReceiver<&'static str>, expected: &str) {
    assert_eq!(
        timeout(GUARD, receiver.recv()).await.expect("probe starts"),
        Some(expected)
    );
}

#[tokio::test]
async fn parallel_sibling_callbacks_overlap_inside_a_serial_parent() {
    let activity = Arc::new(Activity::default());
    let release = Arc::new(Semaphore::new(0));
    let (started, mut receiver) = mpsc::unbounded_channel();
    let parent = RunningParent::start(vec![Arc::new(Probe {
        name: "parallel",
        parallel: true,
        activity: activity.clone(),
        entered: started,
        release: release.clone(),
    })])
    .await;
    let run = tokio::spawn({
        let host = parent.host.clone();
        async move {
            tokio::join!(
                host.execute_tool(call("parallel")),
                host.execute_tool(call("parallel"))
            )
        }
    });
    entered(&mut receiver, "parallel").await;
    entered(&mut receiver, "parallel").await;
    assert_eq!(activity.maximum.load(Ordering::SeqCst), 2);
    release.add_permits(2);
    let (first, second) = timeout(GUARD, run).await.unwrap().unwrap();
    assert!(first.unwrap().ok);
    assert!(second.unwrap().ok);
    assert!(parent.finish().await.ok);
}

#[tokio::test]
async fn serial_sibling_fences_existing_and_later_parallel_callbacks() {
    let activity = Arc::new(Activity::default());
    let (started, mut receiver) = mpsc::unbounded_channel();
    let releases: Vec<_> = (0..3).map(|_| Arc::new(Semaphore::new(0))).collect();
    let tools = [("early", true), ("exclusive", false), ("late", true)]
        .into_iter()
        .zip(&releases)
        .map(|((name, parallel), release)| {
            Arc::new(Probe {
                name,
                parallel,
                activity: activity.clone(),
                entered: started.clone(),
                release: release.clone(),
            }) as Arc<dyn Tool>
        })
        .collect();
    let parent = RunningParent::start(tools).await;
    let early = tokio::spawn({
        let host = parent.host.clone();
        async move { host.execute_tool(call("early")).await }
    });
    entered(&mut receiver, "early").await;
    let mut exclusive = parent.host.execute_tool(call("exclusive"));
    assert!(futures_util::poll!(exclusive.as_mut()).is_pending());
    let mut late = parent.host.execute_tool(call("late"));
    assert!(futures_util::poll!(late.as_mut()).is_pending());
    assert!(receiver.try_recv().is_err());
    releases[0].add_permits(1);
    assert!(timeout(GUARD, early).await.unwrap().unwrap().unwrap().ok);
    tokio::select! {
        biased;
        result = &mut exclusive => panic!("exclusive finished before release: {result:?}"),
        _ = entered(&mut receiver, "exclusive") => {}
        _ = tokio::time::sleep(GUARD) => panic!("exclusive did not acquire its gate"),
    }
    assert!(futures_util::poll!(late.as_mut()).is_pending());
    releases[1].add_permits(1);
    assert!(timeout(GUARD, exclusive).await.unwrap().unwrap().ok);
    releases[2].add_permits(1);
    assert!(timeout(GUARD, late).await.unwrap().unwrap().ok);
    entered(&mut receiver, "late").await;
    assert_eq!(activity.maximum.load(Ordering::SeqCst), 1);
    assert_eq!(
        *activity.events.lock().unwrap(),
        [
            "early:start",
            "early:finish",
            "exclusive:start",
            "exclusive:finish",
            "late:start",
            "late:finish"
        ]
    );
    assert!(parent.finish().await.ok);
}

struct Forward;

#[async_trait]
impl Tool for Forward {
    fn spec(&self) -> ToolSpec {
        spec("forward", false)
    }

    async fn invoke(&self, call: &ToolCall, ctx: ToolContext) -> Result<ToolResult> {
        let mut result = ctx.host.unwrap().execute_tool(self::call("leaf")).await?;
        result.call_id = call.id.clone();
        Ok(result)
    }
}

#[tokio::test]
async fn serial_child_can_await_a_serial_grandchild_without_reacquiring_parent_gate() {
    let activity = Arc::new(Activity::default());
    let (started, mut receiver) = mpsc::unbounded_channel();
    let parent = RunningParent::start(vec![
        Arc::new(Forward),
        Arc::new(Probe {
            name: "leaf",
            parallel: false,
            activity: activity.clone(),
            entered: started,
            release: Arc::new(Semaphore::new(1)),
        }),
    ])
    .await;
    let result = timeout(GUARD, parent.host.execute_tool(call("forward")))
        .await
        .expect("grandchild does not wait for its parent's permit")
        .unwrap();
    assert!(result.ok, "{result:?}");
    entered(&mut receiver, "leaf").await;
    assert_eq!(activity.maximum.load(Ordering::SeqCst), 1);
    assert!(parent.finish().await.ok);
}

#[tokio::test]
async fn cancellation_rejects_a_queued_sibling_before_any_child_effect() {
    let activity = Arc::new(Activity::default());
    let (started, mut receiver) = mpsc::unbounded_channel();
    let release = Arc::new(Semaphore::new(0));
    let parent = RunningParent::start(vec![Arc::new(Probe {
        name: "exclusive",
        parallel: false,
        activity: activity.clone(),
        entered: started,
        release: release.clone(),
    })])
    .await;
    let first = tokio::spawn({
        let host = parent.host.clone();
        async move { host.execute_tool(call("exclusive")).await }
    });
    entered(&mut receiver, "exclusive").await;
    let mut queued = parent.host.execute_tool(call("exclusive"));
    assert!(futures_util::poll!(queued.as_mut()).is_pending());
    parent.cancellation.cancel();
    let error = timeout(GUARD, queued).await.unwrap().unwrap_err();
    assert!(error.to_string().contains("canceled before scheduling"));
    assert_eq!(*activity.events.lock().unwrap(), ["exclusive:start"]);
    assert!(receiver.try_recv().is_err());
    release.add_permits(1);
    timeout(GUARD, first).await.unwrap().unwrap().unwrap();
    parent.finish().await;
}
