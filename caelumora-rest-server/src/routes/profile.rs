//! The logged-in user's own profile (issue 176): how they introduce
//! themselves to the agents of every channel they take part in.

use std::sync::Arc;
use axum::extract::State;
use axum::http::StatusCode;
use caelumora_model::security::SecurityContext;
use caelumora_model::store::{EntityStore, StoreError, StoreResult};
use caelumora_model::user::{UserProfile, MAX_INTRODUCTION_CHARS};
use crate::auth::AuthenticatedUser;
use crate::error::{ApiError, Json};
use crate::models::{ProfileRequest, ProfileResponse};
use crate::state::AppState;

/// The user's profile; one without an introduction if they never wrote one.
pub async fn get_profile(State(state): State<Arc<AppState>>, user: AuthenticatedUser) -> Result<Json<ProfileResponse>, ApiError> {
    let store = state.runtime_manager.entity_store();
    let introduction = match store.get_user_profile(&user.security_context(), user.id).await {
        Ok(profile) => profile.introduction,
        Err(StoreError::NotFound { .. }) => None,
        Err(e) => return Err(ApiError::internal("profile_failed", "reading the profile", &e.into())),
    };
    Ok(Json(ProfileResponse { introduction, max_introduction_chars: MAX_INTRODUCTION_CHARS }))
}

/// Sets the user's introduction; `null` or blank removes it. Answers
/// `400 invalid_introduction` for one that is too long or contains control
/// characters.
pub async fn update_profile(State(state): State<Arc<AppState>>, user: AuthenticatedUser, Json(payload): Json<ProfileRequest>) -> Result<Json<ProfileResponse>, ApiError> {
    let profile = UserProfile::introducing(user.id, payload.introduction.as_deref())
        .map_err(|e| ApiError::new(StatusCode::BAD_REQUEST, "invalid_introduction", e.to_string()))?;
    save(state.runtime_manager.entity_store().as_ref(), &user.security_context(), &profile).await
        .map_err(|e| ApiError::internal("profile_failed", "saving the profile", &e.into()))?;
    Ok(Json(ProfileResponse { introduction: profile.introduction, max_introduction_chars: MAX_INTRODUCTION_CHARS }))
}

/// Stores `profile` as `context`, whether its user has one already or not:
/// changed if there is one, else created - and changed after all if one was
/// created meanwhile. The store asks whether `context` may.
async fn save(store: &dyn EntityStore, context: &SecurityContext, profile: &UserProfile) -> StoreResult<()> {
    match store.update_user_profile(context, profile).await {
        Err(StoreError::NotFound { .. }) => match store.create_user_profile(context, profile).await {
            Err(StoreError::Conflict(_)) => store.update_user_profile(context, profile).await,
            created => created,
        },
        updated => updated,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use caelumora_model::actor::ActorId;
    use caelumora_model::security::OwnershipSecurityManager;
    use caelumora_model::store_impl::memory::MemoryStore;
    use caelumora_model::user::UserId;

    #[tokio::test]
    async fn a_profile_is_created_once_and_changed_after() {
        let store = MemoryStore::with_security_manager(Arc::new(OwnershipSecurityManager::new()));
        let alice = UserId::new();
        let context = SecurityContext::new(ActorId::User(alice), Default::default(), Default::default());
        for text in ["I run the budget.", "I run the budget and the team."] {
            save(&store, &context, &UserProfile::introducing(alice, Some(text)).unwrap()).await.expect("her own");
            assert_eq!(store.get_user_profile(&context, alice).await.unwrap().introduction.as_deref(), Some(text));
        }
    }
}
