//! 抓屏判断 Codex TUI 当前是不是“普通输入框”（docs/CODEX_UI.md §7）。样本：`testdata/codex-<版本>-screen-*.txt`。
//!
//! ```text
//! • Working (6s • esc to interrupt)        ← 运行中的活动行
//!
//! › 输入框里的文字                          ← 最后一个 `› ` 开头的行（空着时是暗淡的 “Ask Codex to do anything”）
//!   续行缩进两格
//!                                           ← 空行
//!   GPT-6-Astra medium · ~\path            ← 状态行
//!   ? for shortcuts                         ← 提示行
//! ```
//!
//! 对话框（信任文件夹、审批、更新提示、`/model` 之类的选择器）底部是 `› 1. …` 这样的编号选项。
//! 注意：刚启动时输入框先画出来，信任文件夹的对话框要过几秒才弹出（实测），所以不能看到一次输入框就当作可以输入。

use super::screen::{strip, Live, Screen};

/// 只看最下面这么多行（再往上是对话历史，里面也可能有 `›`）
const BOTTOM: usize = 24;

/// `› 1. Trust and continue` 这样的选中项
fn is_option(plain: &str) -> bool {
    let rest = plain.trim_start().strip_prefix('›').map(str::trim_start).unwrap_or("");
    let digits = rest.chars().take_while(char::is_ascii_digit).count();
    digits > 0 && rest[digits..].starts_with('.')
}

pub fn classify(capture: &str) -> Screen {
    let lines: Vec<(String, String)> = capture.lines().map(strip).collect();
    let mut end = lines.len();
    while end > 0 && lines[end - 1].0.trim().is_empty() {
        end -= 1;
    }
    let start = end.saturating_sub(BOTTOM);
    let bottom = &lines[start..end];
    // 最后一个 `› ` 开头的行是输入框（`/` 命令的候选也是 `›` 开头，但在输入框上面）。
    // 对话历史里用户发过的消息也是 `› ` 开头：发过 `1. …` 开头的消息时历史里有 `› 1. …`，只看最后一个 `›`
    let Some(i) = bottom.iter().rposition(|(p, _)| p.trim_start().starts_with('›')) else { return Screen::Modal };
    if is_option(&bottom[i].0) {
        return Screen::Modal;
    }
    let mut text = bottom[i].1.strip_prefix('›').unwrap_or("").trim_start().trim_end().to_string();
    for (plain, bright) in &bottom[i + 1..] {
        if plain.trim().is_empty() || !plain.starts_with("  ") {
            break;
        }
        let cont = bright.trim();
        if !cont.is_empty() {
            text.push('\n');
            text.push_str(cont);
        }
    }
    let busy = bottom.iter().any(|(p, _)| p.contains("esc to interrupt"));
    Screen::Input { text, busy }
}

/// TUI 上的选择框（信任文件夹、Codex 自己的确认框、更新提示……）：和 Claude Code 的一样解析，只是光标是 `›`。
pub fn dialog(capture: &str) -> Option<super::screen::Dialog> {
    // 输入框那一行也是 `›` 开头：最后一个 `›` 是带编号的选项（`› 1. …`）才是选择框
    if classify(capture) != Screen::Modal || !capture.lines().rev().map(|l| strip(l).0).find(|l| l.trim_start().starts_with('›')).is_some_and(|l| is_option(&l)) {
        return None;
    }
    super::screen::dialog_with(capture, '›')
}

/// 运行中的活动行，例如 `Working (6s • esc to interrupt)`。正在写的回复第一版不取（Codex 的历史区里分不清写完没写完）。
pub fn live(capture: &str) -> Live {
    let activity = capture
        .lines()
        .rev()
        .take(BOTTOM)
        .map(|l| strip(l).0)
        .find(|l| l.contains("esc to interrupt"))
        .map(|l| l.trim().trim_start_matches('•').trim().to_string());
    Live { activity, text: None }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(name: &str) -> String {
        std::fs::read_to_string(format!("{}/src/chat/testdata/codex-0.159.3-screen-{name}.txt", env!("CARGO_MANIFEST_DIR"))).unwrap()
    }

    #[test]
    fn real_captures() {
        assert_eq!(classify(&sample("idle")), Screen::Input { text: String::new(), busy: false }, "暗淡的占位提示不算输入");
        assert_eq!(
            classify(&sample("typed")),
            Screen::Input { text: "Run one shell command that creates s1.txt containing hi, requesting escalated permissions outside the read-only\nsandbox. Then reply OK.".into(), busy: false }
        );
        assert_eq!(classify(&sample("busy")), Screen::Input { text: String::new(), busy: true });
        assert_eq!(classify(&sample("busy2")), Screen::Input { text: String::new(), busy: true });
        assert_eq!(classify(&sample("done")), Screen::Input { text: String::new(), busy: false });
        assert_eq!(classify(&sample("slash")), Screen::Input { text: "/model".into(), busy: false }, "候选在上面，输入框是最后一个 ›");
        for modal in ["trust", "approval"] {
            assert_eq!(classify(&sample(modal)), Screen::Modal, "{modal}");
        }
    }

    #[test]
    fn trust_dialog() {
        let d = dialog(&sample("trust")).unwrap();
        assert_eq!(d.options.iter().map(|o| o.label.as_str()).collect::<Vec<_>>(), ["Trust and continue", "Quit"]);
        assert_eq!(d.selected, 0);
        let a = dialog(&sample("approval")).unwrap();
        assert_eq!(a.options.len(), 3, "Codex 自己的确认框：{a:?}");
        assert!(a.options[0].label.starts_with("Yes, proceed"));
        for plain in ["idle", "busy"] {
            assert!(dialog(&sample(plain)).is_none(), "{plain}");
        }
    }

    #[test]
    fn activity_line() {
        assert_eq!(live(&sample("busy2")).activity.as_deref(), Some("Working (6s • esc to interrupt)"));
        assert_eq!(live(&sample("done")), Live::default());
    }

    /// 用户发过 `1. …` 开头的消息：历史里的 `› 1. …` 不是选择框
    #[test]
    fn numbered_message_in_history() {
        let s = "› 1. 修一下登录\n  2. 再跑测试\n\n• Working (3s • esc to interrupt)\n\n› Ask Codex to do anything\n\n  GPT-6-Astra medium · ~/proj\n  ? for shortcuts\n";
        assert_eq!(classify(s), Screen::Input { text: "Ask Codex to do anything".into(), busy: true });
        assert!(dialog(s).is_none());
    }

    #[test]
    fn options_and_shells() {
        assert!(is_option("› 1. Yes, proceed (y)"));
        assert!(is_option("  ›  12. Something"));
        assert!(!is_option("› 1st thing"));
        assert!(!is_option("› Ask Codex to do anything"));
        assert_eq!(classify("PS C:\\> "), Screen::Modal, "不是 Codex 的界面");
    }
}
