//! Tauri 命令层：前端能调用的全部接口。
//!
//! 约定：
//! - 命令只做「参数校验 + 调领域逻辑 + 组装视图」，不写业务规则
//! - 返回值都是可序列化的视图对象，密钥类字段永不外传
//! - 长任务（agent 一轮对话）立即返回 turn_id，进展通过 `hub://*` 事件推送

pub mod agent;
pub mod extend;
pub mod file;
pub mod app;
pub mod lesson;
pub mod quiz;
pub mod study;
pub mod topic;
pub mod viewer;
