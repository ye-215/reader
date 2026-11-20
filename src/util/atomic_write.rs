//! 原子文件写：先写同目录临时文件再 rename 覆盖。
//!
//! 直接 `fs::write(path, data)` 会先截断目标文件，若此时进程被强杀或崩溃，
//! 目标文件就残留为半截内容。对正文/目录/搜索这类缓存来说，读取侧没有完整性
//! 校验，半截内容会被当成有效缓存**永久返回**，直到用户手动清缓存——这是数据
//! 正确性问题，不只是"报错"。rename 在同一文件系统上是原子的，因此要么读到
//! 完整的旧内容，要么读到完整的新内容。
//!
//! 临时文件名带 uuid：同一路径的并发写各自写各自的临时文件，最后 rename 者胜出，
//! 不会互相踩踏（后写覆盖是这里期望的语义）。

use std::path::{Path, PathBuf};

/// 同步原子写。用于普通 std::fs 热路径。
pub fn atomic_write_sync(path: impl AsRef<Path>, data: &[u8]) -> std::io::Result<()> {
    let path = path.as_ref();
    let tmp = temp_path(path);
    let result = (|| {
        std::fs::write(&tmp, data)?;
        // Windows 上若目标正被其它句柄打开，rename 可能失败；有限重试。
        rename_with_retry(&tmp, path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

/// 异步原子写。用于 tokio::fs 热路径（章节缓存、目录缓存等）。
pub async fn atomic_write(path: impl AsRef<Path>, data: &[u8]) -> std::io::Result<()> {
    let path = path.as_ref();
    let tmp = temp_path(path);
    let result = async {
        tokio::fs::write(&tmp, data).await?;
        rename_with_retry_async(&tmp, path).await
    }
    .await;
    if result.is_err() {
        let _ = tokio::fs::remove_file(&tmp).await;
    }
    result
}

fn temp_path(path: &Path) -> PathBuf {
    let file_name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "cache".to_string());
    let suffix = format!(".{}.tmp", uuid::Uuid::new_v4().simple());
    path.with_file_name(format!("{file_name}{suffix}"))
}

/// rename 覆盖。Windows 上目标被占用时短暂重试，避免偶发写失败。
fn rename_with_retry(from: &Path, to: &Path) -> std::io::Result<()> {
    let mut last_error = None;
    for _ in 0..5u32 {
        match std::fs::rename(from, to) {
            Ok(()) => return Ok(()),
            Err(error) => {
                last_error = Some(error);
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
        }
    }
    Err(last_error.unwrap_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::Other, "rename failed without an error")
    }))
}

async fn rename_with_retry_async(from: &Path, to: &Path) -> std::io::Result<()> {
    let mut last_error = None;
    for _ in 0..5u32 {
        match tokio::fs::rename(from, to).await {
            Ok(()) => return Ok(()),
            Err(error) => {
                last_error = Some(error);
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        }
    }
    Err(last_error.unwrap_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::Other, "rename failed without an error")
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sync_write_is_atomic_and_overwrites() {
        let dir = std::env::temp_dir().join(format!("reader-atomic-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("value.txt");

        atomic_write_sync(&path, b"first").unwrap();
        atomic_write_sync(&path, b"second").unwrap();

        assert_eq!(std::fs::read(&path).unwrap(), b"second");
        // 临时文件必须被清理干净。
        let leftovers: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(|entry| entry.ok())
            .filter(|entry| entry.file_name().to_string_lossy().contains(".tmp"))
            .collect();
        assert!(leftovers.is_empty(), "残留临时文件: {leftovers:?}");
        let _ = std::fs::remove_dir_all(dir);
    }
}
