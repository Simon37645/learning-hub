//! 统一错误类型。所有 Tauri 命令都返回 `AppResult<T>`，
//! `AppError` 手动实现 `Serialize`，前端拿到的是可直接展示的中文/英文短句。

use serde::{Serialize, Serializer};

#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("文件读写失败：{0}")]
    Io(#[from] std::io::Error),

    #[error("数据解析失败：{0}")]
    Json(#[from] serde_json::Error),

    #[error("网络请求失败：{0}")]
    Http(#[from] reqwest::Error),

    #[error("找不到：{0}")]
    NotFound(String),

    #[error("参数不合法：{0}")]
    Invalid(String),

    #[error("已被策略拒绝：{0}")]
    Denied(String),

    #[error("模型调用失败：{0}")]
    Provider(String),

    #[error("操作已取消")]
    Cancelled,

    #[error("{0}")]
    Other(String),
}

pub type AppResult<T> = Result<T, AppError>;

impl AppError {
    /// 越界路径一律走这里，保证「agent 只能碰工作区里的东西」。
    pub fn escape(path: impl std::fmt::Display) -> Self {
        AppError::Denied(format!("路径越出主题目录：{path}"))
    }

    pub fn invalid(msg: impl Into<String>) -> Self {
        AppError::Invalid(msg.into())
    }

    pub fn other(msg: impl Into<String>) -> Self {
        AppError::Other(msg.into())
    }
}

impl Serialize for AppError {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_string())
    }
}

impl From<anyhow::Error> for AppError {
    fn from(e: anyhow::Error) -> Self {
        AppError::Other(e.to_string())
    }
}

/// 给 io 错误补上「哪个文件出事了」。
pub trait IoContext<T> {
    fn ctx(self, path: impl std::fmt::Display) -> AppResult<T>;
}

impl<T> IoContext<T> for std::io::Result<T> {
    fn ctx(self, path: impl std::fmt::Display) -> AppResult<T> {
        self.map_err(|e| AppError::Io(std::io::Error::new(e.kind(), format!("{path} — {e}"))))
    }
}
