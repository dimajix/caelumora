use anyhow::Result;
use futures::future::ready;
use futures::stream::{once, BoxStream, StreamExt};
use futures::TryStreamExt;
use tokio::sync::mpsc;
use caelumora_model::message::{Message, MessageEvent, MessageProcessor};


pub enum LoopCommand {
    // We pass the oneshot sender inside the command
    HandleRequest {
        request: Vec<Message>,
        // This is the "callback" mechanism
        reply_to: oneshot::Sender<BoxStream<'static, Result<MessageEvent>>>,
    },
}

pub struct LoopRunner {
    pub id: String,
    pub processor: Box<dyn MessageProcessor>,
    req_rx: mpsc::UnboundedReceiver<LoopCommand>,
    req_tx: mpsc::UnboundedSender<LoopCommand>,
}

impl LoopRunner {
    pub async fn send_messages(&self, messages: &[Message]) -> BoxStream<'static, Result<MessageEvent>> {
        // 1. Create the "Return Address"
        let (tx, rx) = oneshot::async_channel();

        // 2. Send the command WITH the return address
        let command = LoopCommand::HandleRequest {
            request: messages.to_vec(),
            reply_to: tx,
        };
        if let Err(e) = self.req_tx.send(command) {
            return once(ready(Err(e)))
                .map_err(|_| anyhow::anyhow!("Failed to send command")) // This is a bit tricky with 'once'
                .boxed();
        }

        // 3. Wait for the response
        match rx.await {
            Ok(result) => result,
            Err(e) => once(ready(Err(e)))
                .map_err(|_| anyhow::anyhow!("Error receiving result"))
                .boxed(),
        }
    }

    pub async fn run(&mut self) {
        while let Some(cmd) = self.req_rx.recv().await {
            match cmd {
                LoopCommand::HandleRequest { request, reply_to} => {
                    let result = self.agent.send_messages(request.as_slice());
                    let _ = reply_to.send(result.await);
                }
            }
        }
    }
}
