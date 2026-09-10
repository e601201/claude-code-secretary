//! 表示用のテキスト整形。
//!
//! Claude の応答本文には markdown が混ざるが、秘書は吹き出しにも全文ビューにも
//! 素のテキストとして出す。経路が 2 つに分かれている(吹き出しは `SessionState::set_speech`、
//! 全文は `src-tauri` の `Core`)ので、削ぐ処理はここに 1 つだけ置いて両方から呼ぶ。

/// 装飾のための markdown 記号を落とす。
///
/// 落とすのは強調(`**`, `*`)・見出し(行頭の `#`)・インラインコード(`` ` ``)だけ。
/// 箇条書きの `-` は「複数項目がある」という意味を運んでいるので残す。
/// コードブロックの ``` も、中身の見せ方が別の問題になるので触らない。
pub fn strip_markdown(text: &str) -> String {
    text.lines()
        .map(strip_line)
        .collect::<Vec<_>>()
        .join("\n")
}

fn strip_line(line: &str) -> String {
    let without_heading = strip_heading(line);
    let without_strong = strip_paired(without_heading, "**");
    let without_emphasis = strip_paired(&without_strong, "*");
    strip_paired(&without_emphasis, "`")
}

/// 行頭の `#` を最大 6 個まで落とす。後ろに空白が続くものだけが見出し。
fn strip_heading(line: &str) -> &str {
    let hashes = line.len() - line.trim_start_matches('#').len();
    if hashes == 0 || hashes > 6 {
        return line;
    }
    let rest = &line[hashes..];
    match rest.strip_prefix(' ') {
        Some(body) => body,
        None => line,
    }
}

/// `delim` で囲まれた対を外す。
///
/// 開き直後と閉じ直前が空白の対は装飾ではないので残す(`2 * 3` のような掛け算や
/// グロブを壊さないため)。閉じが見つからない開きもそのまま残す。
fn strip_paired(line: &str, delim: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut rest = line;
    loop {
        let Some(start) = rest.find(delim) else {
            out.push_str(rest);
            return out;
        };
        let after = &rest[start + delim.len()..];
        let Some(end) = after.find(delim) else {
            out.push_str(rest);
            return out;
        };
        let inner = &after[..end];
        let decorative = !inner.is_empty()
            && !inner.starts_with(char::is_whitespace)
            && !inner.ends_with(char::is_whitespace);
        if decorative {
            out.push_str(&rest[..start]);
            out.push_str(inner);
        } else {
            // 装飾ではないので、開きと中身をそのまま残して先へ進む
            out.push_str(&rest[..start + delim.len() + end]);
        }
        rest = &after[end + delim.len()..];
        if !decorative {
            // 閉じ側は次の対の開きになり得るので戻す
            out.push_str(delim);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_emphasis() {
        assert_eq!(strip_markdown("**送った内容**"), "送った内容");
        assert_eq!(strip_markdown("これは *大事* です"), "これは 大事 です");
    }

    #[test]
    fn strips_headings_at_line_start() {
        assert_eq!(strip_markdown("# 見出し"), "見出し");
        assert_eq!(strip_markdown("### 小見出し"), "小見出し");
        assert_eq!(strip_markdown("行中の # は残す"), "行中の # は残す");
    }

    #[test]
    fn strips_inline_code() {
        assert_eq!(
            strip_markdown("`/tmp/x.txt` を作りました"),
            "/tmp/x.txt を作りました"
        );
    }

    #[test]
    fn keeps_list_markers() {
        assert_eq!(
            strip_markdown("- ファイルは 0 バイト\n- 返信しました"),
            "- ファイルは 0 バイト\n- 返信しました"
        );
    }

    #[test]
    fn leaves_lone_and_spaced_markers_alone() {
        // 掛け算やグロブを壊さない(開き直後・閉じ直前が空白の対は装飾ではない)
        assert_eq!(strip_markdown("2 * 3 * 4"), "2 * 3 * 4");
        // 閉じが無ければそのまま
        assert_eq!(strip_markdown("5 * 2 = 10"), "5 * 2 = 10");
        assert_eq!(strip_markdown("未完の **強調"), "未完の **強調");
    }

    #[test]
    fn handles_multiple_pairs_and_multibyte() {
        assert_eq!(
            strip_markdown("**あ** と **い** と `う`"),
            "あ と い と う"
        );
    }

    #[test]
    fn preserves_line_structure() {
        let input = "Discord への返信を送信しました。\n\n**送った内容**\nREADME は…";
        let want = "Discord への返信を送信しました。\n\n送った内容\nREADME は…";
        assert_eq!(strip_markdown(input), want);
    }

    #[test]
    fn empty_and_plain_text_are_unchanged() {
        assert_eq!(strip_markdown(""), "");
        assert_eq!(strip_markdown("ただの文章です"), "ただの文章です");
    }
}
