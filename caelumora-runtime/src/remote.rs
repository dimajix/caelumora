//! What every component that talks to a remote system over HTTP shares
//! besides its endpoint (`caelumora_model::remote`): how its errors are
//! described, without the URL a library writes into them (issue 188).
//! Public for the component kinds outside the core, such as `a2a_agent`
//! (issue 326).

/// The full chain of the causes of `error`, a failed request, without the
/// URL that `reqwest` writes into its messages.
pub fn describe_request_error(error: reqwest::Error) -> String {
    describe_chain(&error.without_url())
}

/// The full chain of an error's causes as one line, e.g. "error sending
/// request: connection refused", for errors whose own `Display` stops at
/// the outermost layer.
pub fn describe_chain(error: &(dyn std::error::Error + 'static)) -> String {
    let mut parts: Vec<String> = Vec::new();
    let mut current: Option<&(dyn std::error::Error + 'static)> = Some(error);
    while let Some(e) = current {
        let text = e.to_string();
        if !parts.iter().any(|p| p.contains(&text)) {
            parts.push(text);
        }
        current = e.source();
    }
    parts.join(": ")
}
