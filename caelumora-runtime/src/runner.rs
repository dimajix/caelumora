use anyhow::Result;
use caelumora_model::message::{Message, MessageEvent, MessageProcessor};
use futures::future::ready;
use futures::stream::{once, BoxStream, StreamExt};
use futures::TryStreamExt;
use log::{debug, info};
use tokio::sync::mpsc;
use tokio::sync::mpsc::UnboundedReceiver;
use tokio::task::JoinHandle;
use uuid::Uuid;

pub enum LoopCommand {
    // We pass the oneshot sender inside the command
    HandleRequest {
        request: Vec<Message>,
        // This is the "callback" mechanism
        reply_to: tokio::sync::oneshot::Sender<BoxStream<'static, Result<MessageEvent>>>,
    },
}

async fn dispatch(
    mut rx: UnboundedReceiver<LoopCommand>,
    mut processor: Box<dyn MessageProcessor>,
) {
    let id = processor.id();
    let name = processor.name();
    let kind = processor.kind();
    while let Some(cmd) = rx.recv().await {
        match cmd {
            LoopCommand::HandleRequest { request, reply_to } => {
                debug!(
                    "dispatch: handling request for processor id={}, kind={}, name={}",
                    id, kind, name
                );
                let result = processor.process_messages(request);
                let _ = reply_to.send(result.await);
            }
        }
    }
}

#[derive(Debug, Clone)]
pub struct LoopRunner {
    id: Uuid,
    kind: String,
    name: String,
    tx: mpsc::UnboundedSender<LoopCommand>,
}

impl LoopRunner {
    pub fn id(&self) -> Uuid {
        self.id
    }
    pub fn kind(&self) -> String {
        self.kind.clone()
    }
    pub fn name(&self) -> String {
        self.name.clone()
    }

    pub fn spawn(proc: Box<dyn MessageProcessor>) -> (LoopRunner, JoinHandle<()>) {
        let (tx, rx) = mpsc::unbounded_channel();
        let runner = LoopRunner {
            id: proc.id(),
            kind: proc.kind(),
            name: proc.name(),
            tx: tx,
        };
        let task = tokio::spawn(async move {
            dispatch(rx, proc).await;
        });
        info!(
            "spawn: created task for processor id={}, kind={}, name={}",
            runner.id, runner.kind, runner.name
        );
        (runner, task)
    }

    pub async fn send_messages(
        &self,
        messages: Vec<Message>,
    ) -> BoxStream<'static, Result<MessageEvent>> {
        // 1. Create the "Return Address"
        let (tx, rx) = tokio::sync::oneshot::channel();

        // 2. Send the command WITH the return address
        let command = LoopCommand::HandleRequest {
            request: messages,
            reply_to: tx,
        };
        if let Err(e) = self.tx.send(command) {
            return once(ready(Err(e)))
                .map_err(|_| anyhow::anyhow!("Failed to send command")) // This is a bit tricky with 'once'
                .boxed();
        }

        // 3. Wait for the response
        match rx.await {
            Ok(result) => result,
            Err(_e) => once(ready(Err(anyhow::anyhow!("Error receiving result")))).boxed(),
        }
    }
}
