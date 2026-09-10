//! Windows 命令行首个 token 解析（纯逻辑，Linux 可测）。
//!
//! 服务的 `lpBinaryPathName` 是"可执行文件路径 + 启动参数"的整串（可能带引号）；
//! 回滚 / 维护页消费的必须是纯 exe 路径，不能把整串再交给 `create_service`
//! （否则会被当作 exe 再引号一次并重新追加参数，服务永远起不来）。

/// 取命令行第一个 token（即可执行文件路径）：
/// - 以 `"` 开头：返回到下一个 `"` 为止（不含引号）；没有闭合引号则到串尾。
/// - 否则：返回到第一个 ASCII 空白（空格 / Tab / 换行等）为止。
/// - 空输入 → 空串。
pub fn first_token(line: &str) -> &str {
    if let Some(rest) = line.strip_prefix('"') {
        match rest.find('"') {
            Some(end) => &rest[..end],
            None => rest,
        }
    } else {
        let end = line
            .find(|c: char| c.is_ascii_whitespace())
            .unwrap_or(line.len());
        &line[..end]
    }
}
