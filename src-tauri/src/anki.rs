//! AnkiConnect 客户端。
//!
//! Anki 桌面版装了 AnkiConnect 插件后会在本机 8765 端口暴露一个 JSON-RPC 接口。
//! 我们只用到三件事：问它在不在、建牌组、灌笔记。
//!
//! 为什么不直接写 Anki 的数据库文件：Anki 运行时会锁库，直接写有损坏风险；
//! AnkiConnect 是官方推荐的外部写入方式，而且能把「卡片的调度状态」交给 Anki 管。

use crate::error::{AppError, AppResult};
use serde_json::{json, Value};
use std::time::Duration;

pub const DEFAULT_URL: &str = "http://127.0.0.1:8765";
/// AnkiConnect 的接口版本，插件文档要求显式带上
const API_VERSION: u32 = 6;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AnkiStatus {
    pub reachable: bool,
    pub url: String,
    pub version: Option<u32>,
    /// 已有的牌组名，界面上用来提示
    pub decks: Vec<String>,
    pub message: String,
}

/// 发一次 AnkiConnect 请求。Anki 不在时返回可读的中文说明。
pub async fn invoke(
    http: &reqwest::Client,
    url: &str,
    action: &str,
    params: Value,
) -> AppResult<Value> {
    let base = if url.trim().is_empty() { DEFAULT_URL } else { url.trim() };
    let body = json!({
        "action": action,
        "version": API_VERSION,
        "params": params,
    });

    let resp = http
        .post(base)
        .timeout(Duration::from_secs(15))
        .json(&body)
        .send()
        .await
        .map_err(|e| {
            AppError::other(format!(
                "连不上 AnkiConnect（{base}）。请确认 Anki 已打开，并且装了 AnkiConnect 插件（工具 → 插件 → 获取插件 → 代码 2055492159）。原始错误：{e}"
            ))
        })?;

    if !resp.status().is_success() {
        return Err(AppError::other(format!("AnkiConnect 返回 HTTP {}", resp.status())));
    }

    let value: Value = resp.json().await?;
    if let Some(err) = value.get("error").and_then(|e| e.as_str()) {
        if !err.is_empty() {
            return Err(AppError::other(format!("AnkiConnect 报错：{err}")));
        }
    }
    Ok(value.get("result").cloned().unwrap_or(Value::Null))
}

pub async fn status(http: &reqwest::Client, url: &str) -> AnkiStatus {
    let base = if url.trim().is_empty() { DEFAULT_URL } else { url.trim() };
    match invoke(http, base, "version", json!({})).await {
        Ok(v) => {
            let decks = invoke(http, base, "deckNames", json!({}))
                .await
                .ok()
                .and_then(|d| {
                    d.as_array().map(|a| {
                        a.iter()
                            .filter_map(|x| x.as_str().map(String::from))
                            .collect::<Vec<_>>()
                    })
                })
                .unwrap_or_default();
            AnkiStatus {
                reachable: true,
                url: base.to_string(),
                version: v.as_u64().map(|x| x as u32),
                decks,
                message: "已连上 Anki".into(),
            }
        }
        Err(e) => AnkiStatus {
            reachable: false,
            url: base.to_string(),
            version: None,
            decks: Vec::new(),
            message: e.to_string(),
        },
    }
}

/// 确保牌组存在（幂等）。
pub async fn ensure_deck(http: &reqwest::Client, url: &str, deck: &str) -> AppResult<()> {
    invoke(http, url, "createDeck", json!({ "deck": deck })).await?;
    Ok(())
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncOutcome {
    pub added: usize,
    pub skipped: usize,
    pub failed: Vec<String>,
    pub deck: String,
}

/// 把一批卡片灌进 Anki。
///
/// 已经同步过的（带 anki_note_id）会被跳过，避免每次同步都产生重复卡；
/// 需要改内容时应当在 Anki 里改，或先删掉再同步。
pub async fn add_notes(
    http: &reqwest::Client,
    url: &str,
    deck: &str,
    notes: Vec<Value>,
) -> AppResult<Vec<Option<i64>>> {
    if notes.is_empty() {
        return Ok(Vec::new());
    }
    ensure_deck(http, url, deck).await?;
    let result = invoke(
        http,
        url,
        "addNotes",
        json!({ "notes": notes }),
    )
    .await?;
    Ok(result
        .as_array()
        .map(|a| {
            a.iter()
                .map(|v| {
                    if v.is_null() {
                        None
                    } else {
                        v.as_i64()
                    }
                })
                .collect()
        })
        .unwrap_or_default())
}

/// 测试连接（设置页用）。
pub async fn ping(http: &reqwest::Client, url: &str) -> AppResult<String> {
    let v = invoke(http, url, "version", json!({})).await?;
    Ok(format!(
        "AnkiConnect 版本 {}",
        v.as_u64().map(|x| x.to_string()).unwrap_or_else(|| "?".into())
    ))
}
