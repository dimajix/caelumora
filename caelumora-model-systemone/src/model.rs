//! The backend `systemone` (issue 551): a decision model reached through the contract
//! `POST /v1/systemone`, which answers typed questions about one text with probabilities.
//!
//! A request names the `model`, the `state` - the text judged - and 1 to 64 named `questions`; the
//! response answers each by its name. The contract's error format is not documented, so an error is
//! told by its status alone.
//!
//! Parameters: `endpoint`, the server, `http://127.0.0.1:11434` unless written, whose
//! `timeout_secs` bounds a decision, 5 seconds unless written, and whose credential is resolved for
//! whoever asks; `model`, `nimble` unless written; `keep_alive`, how long the server keeps the
//! model loaded after a request, its own default if left out; and `destination`, what the server is
//! cleared for, `public` unless written.

use async_trait::async_trait;
use caelumora_api::component::{service_of, ComponentError, Params};
use caelumora_api::decision::{Answer, Question};
use caelumora_api::error::describe_request_error;
use caelumora_api::model::{ModelError, ModelResult};
use caelumora_domain::confidentiality::Classification;
use caelumora_domain::remote::RemoteEndpoint;
use caelumora_kernel::backend::Backends;
use caelumora_model::DecisionModelProvider;
use caelumora_security::secret::SecretManager;
use caelumora_security::SecurityContext;
use reqwest::{Client, Error};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use std::time::Duration;
use tracing::trace;

/// The type of the backend in the configuration.
const SYSTEMONE_BACKEND: &str = "systemone";

/// Where the contract is served below an endpoint's URL.
const PATH: [&str; 2] = ["v1", "systemone"];

/// How long a decision may take unless the endpoint says otherwise: whoever asks waits for it.
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(5);

/// The parameters of a model of the backend `systemone`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SystemOneModelConfig {
    #[serde(default = "default_endpoint")]
    endpoint: RemoteEndpoint,
    #[serde(default = "default_model")]
    model: String,
    #[serde(default)]
    keep_alive: Option<String>,
    #[serde(default)]
    destination: Option<Classification>,
}

impl Params for SystemOneModelConfig {}

fn default_endpoint() -> RemoteEndpoint {
    RemoteEndpoint::new("http://127.0.0.1:11434")
}

fn default_model() -> String {
    "nimble".to_string()
}

/// A question as the contract writes it.
#[derive(Serialize)]
#[serde(tag = "type", rename_all = "lowercase")]
enum WrittenQuestion<'a> {
    Choice { instructions: &'a str, criteria: &'a BTreeMap<String, Option<String>> },
    Noul { instructions: &'a str },
}

impl<'a> From<&'a Question> for WrittenQuestion<'a> {
    fn from(question: &'a Question) -> Self {
        match question {
            Question::Choice { instructions, criteria } => Self::Choice { instructions, criteria },
            Question::Noul { instructions } => Self::Noul { instructions },
        }
    }
}

/// An answer as the contract writes it.
#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
enum WrittenAnswer {
    Choice { choice: String, probabilities: HashMap<String, f64> },
    Noul { noul: f64 },
}

impl From<WrittenAnswer> for Answer {
    fn from(answer: WrittenAnswer) -> Self {
        match answer {
            WrittenAnswer::Choice { choice, probabilities } => {
                Self::Choice { choice, probabilities }
            }
            WrittenAnswer::Noul { noul } => Self::Noul { noul },
        }
    }
}

#[derive(Serialize)]
struct Request<'a> {
    model: &'a str,
    state: &'a str,
    questions: BTreeMap<&'a str, WrittenQuestion<'a>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    keep_alive: Option<&'a str>,
}

#[derive(Deserialize)]
struct Response {
    answers: HashMap<String, WrittenAnswer>,
}

/// A decision model, served at an endpoint.
#[derive(Debug)]
struct SystemOneModel {
    http: Client,
    endpoint: RemoteEndpoint,
    model: String,
    keep_alive: Option<String>,
    destination: Classification,
    secret_manager: Arc<SecretManager>,
}

impl SystemOneModel {
    /// That the server could not be reached for `reason`, without what would show its credentials.
    fn unreachable(&self, reason: String) -> ModelError {
        ModelError::Connection { endpoint: self.endpoint.shown(), reason }
    }

    /// That the request failed with `e`.
    fn failed(&self, e: Error) -> ModelError {
        self.unreachable(self.endpoint.redact_in(&describe_request_error(e)))
    }

    /// That its answer cannot be read, for `reason`.
    fn invalid(&self, reason: String) -> ModelError {
        ModelError::InvalidResponse { endpoint: self.endpoint.shown(), reason }
    }
}

#[async_trait]
impl DecisionModelProvider for SystemOneModel {
    fn destination(&self) -> &Classification {
        &self.destination
    }

    async fn decide(
        &self,
        asking: &SecurityContext,
        state: &str,
        questions: &BTreeMap<String, Question>,
    ) -> ModelResult<HashMap<String, Answer>> {
        let body = Request {
            model: &self.model,
            state,
            questions: questions.iter().map(|(name, q)| (name.as_str(), q.into())).collect(),
            keep_alive: self.keep_alive.as_deref(),
        };
        trace!(endpoint = %self.endpoint.shown(), "Asking a decision model: {}", serde_json::to_string(&body).unwrap_or_default());
        let url = self
            .endpoint
            .url_of(&PATH)
            .ok_or_else(|| self.unreachable("the endpoint is no URL".to_string()))?;
        let mut request = self.http.post(url).json(&body);
        let token = self
            .endpoint
            .bearer_token(&self.secret_manager.for_context(asking.clone()))
            .await
            .map_err(|e| self.unreachable(format!("its credential is not available: {e}")))?;
        if let Some(token) = token {
            request = request.bearer_auth(token);
        }
        let response = request.send().await.map_err(|e| self.failed(e))?;
        let status = response.status();
        if !status.is_success() {
            return Err(ModelError::Rejected {
                endpoint: self.endpoint.shown(),
                model: self.model.clone(),
                status: status.as_u16(),
                message: "the contract does not document its errors".to_string(),
            });
        }
        let text = response.text().await.map_err(|e| self.failed(e))?;
        let response: Response =
            serde_json::from_str(&text).map_err(|e| self.invalid(e.to_string()))?;
        if let Some(unanswered) =
            questions.keys().find(|name| !response.answers.contains_key(*name))
        {
            return Err(self.invalid(format!("the question '{unanswered}' is not answered")));
        }
        Ok(response.answers.into_iter().map(|(name, answer)| (name, answer.into())).collect())
    }
}

/// Registers the backend `systemone` of the decision models with `backends`.
pub(crate) fn register_backend(backends: &mut Backends) {
    backends.register::<dyn DecisionModelProvider, SystemOneModelConfig, _>(
        SYSTEMONE_BACKEND,
        |_, params, services| {
            let http = Client::builder()
                .timeout(params.endpoint.timeout_or(DEFAULT_TIMEOUT))
                .build()
                .map_err(|e| ComponentError::InvalidConfig(describe_request_error(e)))?;
            Ok(Arc::new(SystemOneModel {
                http,
                endpoint: params.endpoint,
                model: params.model,
                keep_alive: params.keep_alive,
                destination: params.destination.unwrap_or(Classification::PUBLIC),
                secret_manager: Arc::clone(service_of::<SecretManager>(services)?),
            }))
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::extract::State;
    use axum::http::{HeaderMap, StatusCode};
    use axum::routing::post;
    use axum::{Json, Router};
    use caelumora_domain::secret::SecretResult;
    use caelumora_security::secret::SecretProvider;
    use caelumora_security::AllowAllPolicy;
    use serde_json::{json, Value};
    use std::sync::Mutex;
    use tokio::net::TcpListener;

    /// A secret provider that knows one secret.
    #[derive(Debug)]
    struct OneSecret;

    #[async_trait]
    impl SecretProvider for OneSecret {
        async fn get_secret(&self, key: &str) -> SecretResult<Option<String>> {
            Ok((key == "DECIDE").then(|| "the-token".to_string()))
        }
    }

    /// A request a server was asked, with the credential it came with.
    type Asked = (Value, Option<String>);

    /// A server that answers every request with `answer` and `status`, and keeps what it was
    /// asked.
    #[derive(Clone)]
    struct FakeServer {
        status: StatusCode,
        answer: Value,
        asked: Arc<Mutex<Vec<Asked>>>,
    }

    impl FakeServer {
        fn answering(answer: Value) -> Self {
            Self { status: StatusCode::OK, answer, asked: Arc::default() }
        }

        fn failing(status: StatusCode) -> Self {
            Self { status, answer: Value::Null, asked: Arc::default() }
        }

        fn asked(&self) -> Vec<Asked> {
            self.asked.lock().unwrap().clone()
        }
    }

    /// Serves `fake`; its URL.
    async fn serve(fake: FakeServer) -> String {
        let app = Router::new()
            .route(
                "/v1/systemone",
                post(
                    |State(fake): State<FakeServer>,
                     headers: HeaderMap,
                     Json(body): Json<Value>| async move {
                        let credential = headers
                            .get("authorization")
                            .map(|value| value.to_str().unwrap().to_string());
                        fake.asked.lock().unwrap().push((body, credential));
                        (fake.status, Json(fake.answer.clone()))
                    },
                ),
            )
            .with_state(fake);
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await });
        url
    }

    /// The model `params` configure, each read as the kernel reads them.
    fn model(params: Value) -> SystemOneModel {
        let params = SystemOneModelConfig::read(&params).unwrap();
        SystemOneModel {
            http: Client::new(),
            endpoint: params.endpoint,
            model: params.model,
            keep_alive: params.keep_alive,
            destination: params.destination.unwrap_or(Classification::PUBLIC),
            secret_manager: Arc::new(SecretManager::new(
                vec![Arc::new(OneSecret)],
                Arc::new(AllowAllPolicy::new()),
            )),
        }
    }

    fn at(url: &str) -> Value {
        json!({ "endpoint": { "url": url } })
    }

    fn question() -> BTreeMap<String, Question> {
        let instructions = "Is it so?".to_string();
        BTreeMap::from([("fits".to_string(), Question::Noul { instructions })])
    }

    /// An answer to `question`, as the contract writes it, with what it does not need besides.
    fn answered() -> Value {
        json!({ "model": "nimble", "usage": {}, "answers": { "fits": { "type": "noul", "noul": 0.7 } } })
    }

    async fn asked(model: &SystemOneModel) -> ModelResult<HashMap<String, Answer>> {
        model.decide(&SecurityContext::default(), "It is so.", &question()).await
    }

    #[tokio::test]
    async fn a_model_is_asked_by_the_contract_and_its_answers_are_read() {
        let fake = FakeServer::answering(answered());
        let url = serve(fake.clone()).await;
        let mut params = at(&url);
        params["model"] = json!("nimble:9b");
        params["keep_alive"] = json!("5m");

        let answers = asked(&model(params)).await.unwrap();

        assert_eq!(answers["fits"], Answer::Noul { noul: 0.7 });
        assert_eq!(
            fake.asked(),
            [(
                json!({
                    "model": "nimble:9b",
                    "state": "It is so.",
                    "keep_alive": "5m",
                    "questions": { "fits": { "type": "noul", "instructions": "Is it so?" } },
                }),
                None
            )]
        );
    }

    #[tokio::test]
    async fn what_is_not_written_is_not_sent_and_its_defaults_hold() {
        let fake = FakeServer::answering(answered());
        let model = model(at(&serve(fake.clone()).await));

        asked(&model).await.unwrap();

        assert_eq!(fake.asked()[0].0["model"], "nimble");
        assert!(fake.asked()[0].0.get("keep_alive").is_none());
        assert_eq!(model.destination(), &Classification::PUBLIC);
    }

    /// Issue 551: a server may take a credential, which is resolved for whoever asks.
    #[tokio::test]
    async fn the_credential_of_the_endpoint_is_sent_and_one_not_there_ends_the_call() {
        let fake = FakeServer::answering(answered());
        let url = serve(fake.clone()).await;
        let with = |secret: &str| {
            model(
                json!({ "endpoint": { "url": url, "auth": { "type": "bearer", "secret": secret } } }),
            )
        };

        asked(&with("DECIDE")).await.unwrap();
        assert_eq!(fake.asked()[0].1.as_deref(), Some("Bearer the-token"));

        let missing = asked(&with("OTHER")).await.unwrap_err();
        assert!(matches!(missing, ModelError::Connection { .. }), "{missing}");
        assert_eq!(fake.asked().len(), 1, "nothing sent without it");
    }

    #[tokio::test]
    async fn a_server_that_refuses_or_leaves_a_question_unanswered_gives_no_answers() {
        let url = serve(FakeServer::failing(StatusCode::INTERNAL_SERVER_ERROR)).await;
        let refused = asked(&model(at(&url))).await.unwrap_err();
        assert!(matches!(refused, ModelError::Rejected { status: 500, .. }), "{refused}");

        let url = serve(FakeServer::answering(json!({ "answers": {} }))).await;
        let unanswered = asked(&model(at(&url))).await.unwrap_err();
        assert!(unanswered.to_string().contains("'fits'"), "{unanswered}");

        let unreachable = asked(&model(at("http://user:secret@127.0.0.1:1"))).await.unwrap_err();
        assert!(!unreachable.to_string().contains("secret"), "{unreachable}");
    }

    #[test]
    fn its_parameters_are_checked() {
        let checked = |params: Value| SystemOneModelConfig::read(&params).map(drop);
        assert!(checked(json!({})).is_ok(), "every one has a default");
        assert!(checked(json!({ "destination": { "level": "internal" } })).is_ok());
        assert!(checked(json!({ "temperature": 0.1 })).is_err(), "nothing unknown");
    }
}
