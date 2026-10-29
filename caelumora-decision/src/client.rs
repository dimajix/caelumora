//! A client of the decision-model contract `POST /v1/systemone` (issue
//! 403): typed questions about one text, answered with probabilities.
//!
//! A request names the `model`, the `state` - the text judged - and 1 to 64
//! named `questions`; the response answers each by its name. Of the three
//! kinds of question the contract knows, the client asks two: `choice`, one
//! of 2 to 26 options, and `noul`, yes or no. Its error format is not
//! documented, so an error is told by its status alone.

use caelumora_model::remote::RemoteEndpoint;
use caelumora_model::secret::{SecretError, SecretManager};
use caelumora_model::security::SecurityContext;
use caelumora_runtime::remote::describe_request_error;
use reqwest::{Client, Error};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use std::time::Duration;
use tracing::trace;

/// Where the contract is served below an endpoint's URL.
const PATH: &str = "v1/systemone";

/// How long a decision may take unless the endpoint says otherwise: the
/// channel waits for it before it routes anything else.
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(5);

/// The most options a `choice` may have.
pub const MAX_OPTIONS: usize = 26;

/// The most questions one request may ask.
pub const MAX_QUESTIONS: usize = 64;

/// Why a decision model gave no answer. Its messages name the endpoint
/// without credentials and nothing of the text judged.
#[derive(Debug, thiserror::Error)]
pub enum DecisionError {
    #[error("the decision model at {endpoint} cannot be reached: {reason}")]
    Unreachable { endpoint: String, reason: String },
    #[error("the decision model at {endpoint} refused the request: HTTP {status}")]
    Rejected { endpoint: String, status: u16 },
    #[error("the decision model at {endpoint} answered what cannot be read: {reason}")]
    InvalidResponse { endpoint: String, reason: String },
    #[error("the credential of the decision model at {endpoint} is not available: {source}")]
    Credential { endpoint: String, source: SecretError },
}

pub type DecisionResult<T> = Result<T, DecisionError>;

/// A question about the state.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Question {
    /// Which one of the options fits: each named, with what it means -
    /// `None` where its name says it.
    Choice { instructions: String, criteria: BTreeMap<String, Option<String>> },
    /// Whether what `instructions` asks is true.
    Noul { instructions: String },
}

/// The answer to a question.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Answer {
    /// The option chosen, and how probable each option is.
    Choice { choice: String, probabilities: HashMap<String, f64> },
    /// How probable it is that the answer is yes.
    Noul { noul: f64 },
}

#[derive(Serialize)]
struct Request<'a> {
    model: &'a str,
    state: &'a str,
    questions: &'a BTreeMap<String, Question>,
    #[serde(skip_serializing_if = "Option::is_none")]
    keep_alive: Option<&'a str>,
}

#[derive(Deserialize)]
struct Response {
    answers: HashMap<String, Answer>,
}

/// A decision model, served at an endpoint.
#[derive(Debug)]
pub struct DecisionClient {
    http: Client,
    endpoint: RemoteEndpoint,
    model: String,
    /// How long the model stays loaded after a request; the server's
    /// default if `None`.
    keep_alive: Option<String>,
    secret_manager: Arc<SecretManager>,
}

impl DecisionClient {
    /// The model `model` at `endpoint`, whose credential, if it names one,
    /// `secret_manager` resolves. A request may take as long as the
    /// endpoint's timeout says, 5 s unless it says otherwise.
    ///
    /// # Errors
    /// `Unreachable` if no HTTP client can be made.
    pub fn new(
        endpoint: RemoteEndpoint,
        model: String,
        keep_alive: Option<String>,
        secret_manager: Arc<SecretManager>,
    ) -> DecisionResult<Self> {
        let http = Client::builder()
            .timeout(endpoint.timeout_or(DEFAULT_TIMEOUT))
            .build()
            .map_err(|e| DecisionError::Unreachable {
                endpoint: endpoint.shown(),
                reason: describe_request_error(e),
            })?;
        Ok(Self { http, endpoint, model, keep_alive, secret_manager })
    }

    /// The endpoint, for messages: without credentials.
    pub fn shown(&self) -> String {
        self.endpoint.shown()
    }

    /// The answers to `questions` about `state`, by the name of each
    /// question, asked on behalf of `context`, in which the credential is
    /// resolved.
    ///
    /// # Errors
    /// `Credential` if the credential cannot be resolved; `Unreachable` if
    /// the endpoint does not answer in time; `Rejected` if it refuses;
    /// `InvalidResponse` if its answer cannot be read or leaves a question
    /// unanswered.
    pub async fn ask(
        &self,
        context: &SecurityContext,
        state: &str,
        questions: &BTreeMap<String, Question>,
    ) -> DecisionResult<HashMap<String, Answer>> {
        let body = Request {
            model: &self.model,
            state,
            questions,
            keep_alive: self.keep_alive.as_deref(),
        };
        trace!(endpoint = %self.shown(), "Asking a decision model: {}", serde_json::to_string(&body).unwrap_or_default());
        let url = format!("{}/{PATH}", self.endpoint.url.trim_end_matches('/'));
        let mut request = self.http.post(url).json(&body);
        let token = self
            .endpoint
            .bearer_token(&self.secret_manager, context)
            .await
            .map_err(|source| DecisionError::Credential { endpoint: self.shown(), source })?;
        if let Some(token) = token {
            request = request.bearer_auth(token);
        }
        let response = request.send().await.map_err(|e| self.unreachable(e))?;
        let status = response.status();
        if !status.is_success() {
            return Err(DecisionError::Rejected {
                endpoint: self.shown(),
                status: status.as_u16(),
            });
        }
        let text = response.text().await.map_err(|e| self.unreachable(e))?;
        let response: Response =
            serde_json::from_str(&text).map_err(|e| self.invalid(e.to_string()))?;
        if let Some(unanswered) =
            questions.keys().find(|name| !response.answers.contains_key(*name))
        {
            return Err(self.invalid(format!("the question '{unanswered}' is not answered")));
        }
        Ok(response.answers)
    }

    fn unreachable(&self, e: Error) -> DecisionError {
        DecisionError::Unreachable {
            endpoint: self.shown(),
            reason: self.endpoint.redact_in(&describe_request_error(e)),
        }
    }

    fn invalid(&self, reason: String) -> DecisionError {
        DecisionError::InvalidResponse { endpoint: self.shown(), reason }
    }
}
