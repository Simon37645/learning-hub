//! 图片附件：用户贴进对话里的图落在磁盘上，消息里**只存路径**。
//!
//! 为什么不把 base64 直接写进 `chats/<id>.jsonl`：
//! 1. 那个文件每一行都必须是一条 `ChatMessage`，而且会被整份读进上下文预算、被全文检索扫、
//!    被用户用记事本打开——一张 1MB 的截图会让这三件事一起变肿；
//! 2. 图片是二进制，base64 进 JSONL 之后既不能 grep 也不能 diff，
//!    「用别的编辑器改对话」这件事也就废了。
//!
//! 所以磁盘上放原文件（`<工作区>/.hub/attachments/`），消息里放**工作区相对路径**，
//! 和资料、笔记一个思路。发请求时才读盘、按协议编码成 base64——
//! 服务商要的是内联 base64 或 URL，这一步躲不掉，但可以只在这一步付代价。
//!
//! 放在工作区顶层而不是主题里：对话可以在主题之间搬（`chat_move`），
//! 附件跟着工作区走才不会因为搬一次对话就找不到图。

use crate::error::{AppError, AppResult};
use std::path::PathBuf;

/// 单张图的上限。前端贴图前会先缩到长边 1568px，正常截图远达不到这个数；
/// 设上限只是为了防止有人把 200MB 的图塞进来。
pub const MAX_IMAGE_BYTES: u64 = 8 * 1024 * 1024;

/// 附件目录（工作区相对路径），消息里存的就是这个前缀下的路径。
pub const DIR_REL: &str = ".hub/attachments";

/// 读盘拿到的图片内容。
#[derive(Debug, Clone)]
pub struct LoadedImage {
    pub media_type: String,
    pub data: Vec<u8>,
}

/// 落盘后的图片信息。
#[derive(Debug, Clone)]
pub struct StoredImage {
    /// 工作区相对路径，例如 `.hub/attachments/1f3c….png`
    pub rel: String,
    pub media_type: String,
    pub bytes: u64,
}

impl StoredImage {
    /// 转成 base64 data URL（OpenAI 兼容端点要这个形状）。
    pub fn data_url(&self, data: &[u8]) -> String {
        format!("data:{};base64,{}", self.media_type, b64_encode(data))
    }
}

/// 附件读写。只认工作区根，别处不碰。
#[derive(Debug, Clone, Default)]
pub struct Attachments {
    ws_root: PathBuf,
}

impl Attachments {
    pub fn new(ws_root: impl Into<PathBuf>) -> Self {
        Self { ws_root: ws_root.into() }
    }

    /// 没有工作区时的空实现（`complete_once` 这类流程用不到图片）。
    pub fn disabled() -> Self {
        Self::default()
    }

    pub fn is_enabled(&self) -> bool {
        !self.ws_root.as_os_str().is_empty()
    }

    /// 附件目录的绝对路径。
    pub fn dir(&self) -> PathBuf {
        self.ws_root.join(".hub").join("attachments")
    }

    pub fn ensure_dir(&self) -> AppResult<()> {
        if !self.is_enabled() {
            return Err(AppError::other("没有配置工作区，无法保存图片"));
        }
        crate::paths::ensure_dir(&self.dir())
    }

    /// 图片的绝对路径。`rel` 必须是附件目录里的相对路径——
    /// 名校验比事后道歉便宜：越界的路径一律拒绝（消息文件是用户可以手改的）。
    pub fn abs(&self, rel: &str) -> AppResult<PathBuf> {
        if !self.is_enabled() {
            return Err(AppError::other("没有配置工作区，无法读取图片"));
        }
        let cleaned = rel.trim().trim_start_matches('/').replace('\\', "/");
        if !cleaned.starts_with(&format!("{DIR_REL}/")) {
            return Err(AppError::Denied(format!(
                "只允许读取 {DIR_REL}/ 下的附件：{rel}"
            )));
        }
        // `..` 明确拒绝，而不是靠「只取最后一段」把它消掉：
        // 静默换个文件比报错更糟——用户会看到另一张图，却不知道为什么。
        if cleaned.contains("..") {
            return Err(AppError::Denied(format!("附件路径不合法：{rel}")));
        }
        let name = cleaned.rsplit('/').next().unwrap_or_default().to_string();
        if name.is_empty() {
            return Err(AppError::invalid("附件路径不合法"));
        }
        let path = self.dir().join(name);
        if !crate::paths::is_within(&self.dir(), &path) {
            return Err(AppError::Denied("附件路径越界".into()));
        }
        Ok(path)
    }

    /// 存一张图。返回它在消息里该记的路径。
    ///
    /// 类型不认前端报的，认**字节里的魔数**：这样即使有人把 `.txt` 改名成 `.png` 塞进来，
    /// 也不会带着一个假 media_type 发给服务商（那会得到一个很难懂的 400）。
    ///
    /// 原文件名不参与路径（用户起的名字可能带斜杠、中文标点，甚至重名），
    /// 它只由调用方写进消息块里用于显示。
    pub fn save(&self, _orig_name: &str, data: &[u8]) -> AppResult<StoredImage> {
        self.ensure_dir()?;
        if data.is_empty() {
            return Err(AppError::invalid("这张图是空的"));
        }
        if data.len() as u64 > MAX_IMAGE_BYTES {
            return Err(AppError::invalid(format!(
                "图片太大了（{}），上限 {}",
                crate::paths::human_size(data.len() as u64),
                crate::paths::human_size(MAX_IMAGE_BYTES)
            )));
        }
        let media_type = detect_media_type(data).ok_or_else(|| {
            AppError::invalid("只支持 PNG / JPEG / GIF / WebP 四种图片")
        })?;
        let id = uuid::Uuid::new_v4().simple().to_string();
        let file_name = format!("{id}.{}", ext_for(media_type));
        let path = self.dir().join(&file_name);
        crate::store::atomic_write(&path, data)?;
        Ok(StoredImage {
            rel: format!("{DIR_REL}/{file_name}"),
            media_type: media_type.to_string(),
            bytes: data.len() as u64,
        })
    }

    /// 删掉一张图（发送中途失败时回滚用）。删不掉也不报错——
    /// 调用方正忙着返回一个更重要的错误。
    pub fn remove(&self, rel: &str) {
        if let Ok(path) = self.abs(rel) {
            let _ = std::fs::remove_file(path);
        }
    }

    /// 读回一张图。读不到就让调用方决定怎么降级（对模型说话时降级成一行文字）。
    pub fn load(&self, rel: &str) -> AppResult<LoadedImage> {
        let path = self.abs(rel)?;
        let meta = std::fs::metadata(&path)
            .map_err(|_| AppError::NotFound(format!("图片已经不在了：{rel}")))?;
        if meta.len() > MAX_IMAGE_BYTES {
            return Err(AppError::invalid(format!(
                "图片超过上限（{}）",
                crate::paths::human_size(meta.len())
            )));
        }
        let data = std::fs::read(&path).map_err(|e| crate::store::io_err(&path, e))?;
        let media_type = detect_media_type(&data).unwrap_or("image/png").to_string();
        Ok(LoadedImage { media_type, data })
    }
}

/// 从字节判断图片类型——只认这两个协议都支持的四种。
pub fn detect_media_type(data: &[u8]) -> Option<&'static str> {
    if data.starts_with(&[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a]) {
        return Some("image/png");
    }
    if data.starts_with(&[0xff, 0xd8, 0xff]) {
        return Some("image/jpeg");
    }
    if data.starts_with(b"GIF87a") || data.starts_with(b"GIF89a") {
        return Some("image/gif");
    }
    if data.len() > 12 && data.starts_with(b"RIFF") && &data[8..12] == b"WEBP" {
        return Some("image/webp");
    }
    None
}

pub fn ext_for(media_type: &str) -> &'static str {
    match media_type {
        "image/jpeg" => "jpg",
        "image/gif" => "gif",
        "image/webp" => "webp",
        _ => "png",
    }
}

/// 一张图大概占多少 token（**估算**，只用于界面上显示用量与裁剪上下文）。
///
/// 两个服务商都是按「切成 512/768 像素的块」计费的，换算下来每块约 750 像素面积 1 token。
/// 这里不追求精确：它的用途是让「上下文构成」里那张图不是 0，以及让带图的历史能被裁掉。
pub fn estimate_image_tokens(width: Option<u32>, height: Option<u32>) -> u32 {
    let (w, h) = match (width, height) {
        (Some(w), Some(h)) if w > 0 && h > 0 => (w as u64, h as u64),
        // 没量到尺寸时按一张普通截图估
        _ => return 1100,
    };
    ((w * h) / 750).clamp(85, 1600) as u32
}

// ============================================================ base64
//
// 手写而不是引 crate：只用得到的就这四十行，且这里不想为了一个编码表
// 再多一个依赖（构建环境不一定能拉包）。解码宽容一点——
// 允许换行、空格、以及误传进来的 `data:image/png;base64,` 前缀。

const B64_TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

pub fn b64_encode(data: &[u8]) -> String {
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = *chunk.get(1).unwrap_or(&0) as u32;
        let b2 = *chunk.get(2).unwrap_or(&0) as u32;
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(B64_TABLE[(n >> 18) as usize & 63] as char);
        out.push(B64_TABLE[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 {
            B64_TABLE[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            B64_TABLE[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

pub fn b64_decode(raw: &str) -> AppResult<Vec<u8>> {
    let s = match raw.find("base64,") {
        Some(i) => &raw[i + 7..],
        None => raw,
    };
    let mut out: Vec<u8> = Vec::with_capacity(s.len() / 4 * 3 + 3);
    let mut acc: u32 = 0;
    let mut bits: u32 = 0;
    for ch in s.bytes() {
        let v = match ch {
            b'A'..=b'Z' => ch - b'A',
            b'a'..=b'z' => ch - b'a' + 26,
            b'0'..=b'9' => ch - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            // 换行、空格、'='（填充）都不参与计算
            b'\n' | b'\r' | b' ' | b'\t' | b'=' => continue,
            _ => {
                return Err(AppError::invalid(format!(
                    "图片数据不是合法的 base64（遇到字符 {:?}）",
                    ch as char
                )))
            }
        };
        acc = (acc << 6) | v as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push(((acc >> bits) & 0xff) as u8);
        }
    }
    // 空输入返回空字节，而不是报错：「这张图是空的」在 `save` 里报更清楚，
    // 那里才知道用户是想干嘛（贴图），而这里只负责解码。
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_roundtrip_matches_known_values() {
        // 与 RFC 4648 的测试向量对齐，避免自己写的编码表悄悄错一位
        assert_eq!(b64_encode(b""), "");
        assert_eq!(b64_encode(b"f"), "Zg==");
        assert_eq!(b64_encode(b"fo"), "Zm8=");
        assert_eq!(b64_encode(b"foo"), "Zm9v");
        assert_eq!(b64_encode(b"foob"), "Zm9vYg==");
        assert_eq!(b64_encode(b"fooba"), "Zm9vYmE=");
        assert_eq!(b64_encode(b"foobar"), "Zm9vYmFy");

        for s in ["", "f", "fo", "foo", "foob", "fooba", "foobar"] {
            assert_eq!(b64_decode(&b64_encode(s.as_bytes())).unwrap(), s.as_bytes());
        }
        // 二进制（含 0x00 / 0xff）也要能原样往返
        let bin: Vec<u8> = (0..=255u8).collect();
        assert_eq!(b64_decode(&b64_encode(&bin)).unwrap(), bin);
    }

    #[test]
    fn base64_decode_is_lenient() {
        // 前端有时直接给 data URL；中间夹换行也不该报错
        assert_eq!(b64_decode("data:image/png;base64,Zm9v").unwrap(), b"foo");
        assert_eq!(b64_decode("Zm\n9v").unwrap(), b"foo");
        assert!(b64_decode("!!!!").is_err());
        // 空输入不是「坏数据」，只是空的（是不是能当图片用由 save 判断）
        assert!(b64_decode("").unwrap().is_empty());
    }

    #[test]
    fn detects_only_supported_types() {
        assert_eq!(
            detect_media_type(&[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, 0x00]),
            Some("image/png")
        );
        assert_eq!(detect_media_type(&[0xff, 0xd8, 0xff, 0xe0]), Some("image/jpeg"));
        assert_eq!(detect_media_type(b"GIF89a....."), Some("image/gif"));
        let mut webp = b"RIFF\0\0\0\0WEBP".to_vec();
        webp.extend_from_slice(b"VP8 ");
        assert_eq!(detect_media_type(&webp), Some("image/webp"));
        // SVG / 纯文本 / 空
        assert_eq!(detect_media_type(b"<svg xmlns=\"...\">"), None);
        assert_eq!(detect_media_type(b"hello"), None);
        assert_eq!(detect_media_type(b""), None);
    }

    /// 目录名里带中文、路径里带 `..` 都不能越界。
    #[test]
    fn refuses_paths_outside_attachment_dir() {
        let tmp = std::env::temp_dir().join(format!("lh-att-{}", uuid::Uuid::new_v4()));
        let att = Attachments::new(&tmp);
        assert!(att.abs(".hub/attachments/a.png").is_ok());
        assert!(att.abs("../secret.png").is_err());
        assert!(att.abs(".hub/memory/memories.jsonl").is_err());
        assert!(att.abs("notes/a.png").is_err());
        assert!(att.abs("/etc/passwd").is_err());
        assert!(att.abs(".hub/attachments/../../x.png").is_err());
    }

    #[test]
    fn saves_and_loads_a_png() {
        let tmp = std::env::temp_dir().join(format!("lh-att-{}", uuid::Uuid::new_v4()));
        let att = Attachments::new(&tmp);
        let png = {
            let mut v = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
            v.extend_from_slice(b"fake-but-magical");
            v
        };
        let stored = att.save("截图 2026-10-08.png", &png).unwrap();
        assert_eq!(stored.media_type, "image/png");
        assert!(stored.rel.starts_with(".hub/attachments/"));
        assert_eq!(stored.data_url(&png).chars().take(22).collect::<String>(), "data:image/png;base64,");

        let back = att.load(&stored.rel).unwrap();
        assert_eq!(back.data, png);
        assert_eq!(back.media_type, "image/png");

        // 不是图片的字节不许存（免得把假 media_type 发给服务商）
        assert!(att.save("x.txt", b"just text").is_err());
        // 空图也不许
        assert!(att.save("x.png", b"").is_err());

        std::fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn image_token_estimate_is_bounded() {
        assert_eq!(estimate_image_tokens(None, None), 1100);
        assert!(estimate_image_tokens(Some(100), Some(100)) >= 85);
        assert_eq!(estimate_image_tokens(Some(4000), Some(4000)), 1600);
        assert!(estimate_image_tokens(Some(1568), Some(882)) > 1000);
    }
}
