pub use gsm_core::platforms::webchat::conversation::{
    Activity, ActivityPage, Attachment, ChannelAccount, ConversationAccount, ConversationStore,
    InMemoryConversationStore, MAX_ACTIVITY_HISTORY, SharedConversationStore, StoreError,
    StoredActivity, memory_store, noop_store,
};

#[cfg(feature = "store_sqlite")]
pub use gsm_core::platforms::webchat::conversation::sqlite_store;

#[cfg(feature = "store_redis")]
pub use gsm_core::platforms::webchat::conversation::redis_store;
