use serde::{Deserialize, Serialize};
use std::fmt;

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FailureKind {
    Authentication,
    Network,
    MissingFolder,
    DeletionGuard,
    Busy,
    NeedsConfirmation,
    Conflict,
    Configuration,
    Retryable,
    Other,
}

#[derive(Debug)]
pub struct SyncFailure {
    pub kind: FailureKind,
    pub message: String,
}
impl SyncFailure {
    pub fn new(kind: FailureKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }
}
impl fmt::Display for SyncFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.message.fmt(f)
    }
}
impl std::error::Error for SyncFailure {}
pub fn failure_kind(error: &anyhow::Error) -> FailureKind {
    for cause in error.chain() {
        if let Some(failure) = cause.downcast_ref::<SyncFailure>() {
            return failure.kind;
        }
        if let Some(git) = cause.downcast_ref::<git2::Error>() {
            if git.code() == git2::ErrorCode::Auth {
                return FailureKind::Authentication;
            }
            if git.code() == git2::ErrorCode::Locked {
                return FailureKind::Busy;
            }
            if git.code() == git2::ErrorCode::Timeout {
                return FailureKind::Network;
            }
            if matches!(
                git.class(),
                git2::ErrorClass::Net
                    | git2::ErrorClass::Http
                    | git2::ErrorClass::Ssh
                    | git2::ErrorClass::Ssl
            ) {
                return FailureKind::Network;
            }
        }
    }
    FailureKind::Other
}
impl FailureKind {
    pub fn stops_retry(self) -> bool {
        matches!(
            self,
            Self::Authentication
                | Self::MissingFolder
                | Self::DeletionGuard
                | Self::NeedsConfirmation
                | Self::Conflict
                | Self::Configuration
        )
    }
}
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SyncPhase {
    Collect,
    Fetch,
    Reconcile,
    Write,
    Push,
    Mirrors,
    Finished,
}
impl SyncPhase {
    pub fn label(self) -> &'static str {
        match self {
            Self::Collect => "正在收集改动",
            Self::Fetch => "正在获取远端",
            Self::Reconcile => "正在合并版本",
            Self::Write => "正在写回文件",
            Self::Push => "正在上传改动",
            Self::Mirrors => "正在更新备份",
            Self::Finished => "同步完成",
        }
    }
}
#[derive(Clone, Debug, Serialize)]
pub struct ChangeEntry {
    pub path: String,
    pub kind: String,
}
