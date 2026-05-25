use nitpick_agent_core::ActivityStatus;

pub(crate) fn label(value: impl std::fmt::Display) -> String {
    colorize(
        value,
        anstyle::Style::new().effects(anstyle::Effects::DIMMED),
    )
}

pub(crate) fn status_lower(status: &ActivityStatus) -> String {
    colorize(
        format!("{status:?}").to_ascii_lowercase(),
        status_style(status),
    )
}

pub(crate) fn status_title(status: &ActivityStatus) -> String {
    colorize(format!("{status:?}"), status_style(status))
}

pub(crate) fn status_plain_title(status: &ActivityStatus) -> String {
    format!("{status:?}")
}

pub(crate) fn success(value: impl std::fmt::Display) -> String {
    colorize(value, anstyle::AnsiColor::Green.on_default())
}

pub(crate) fn error(value: impl std::fmt::Display) -> String {
    colorize(value, anstyle::AnsiColor::Red.on_default())
}

pub fn format_error_message(message: &str) -> String {
    message
        .lines()
        .map(|line| match line.strip_prefix("error:") {
            Some(rest) => format!("{}{}", error("error:"), rest),
            None => line.to_owned(),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

pub(crate) fn warn(value: impl std::fmt::Display) -> String {
    colorize(value, anstyle::AnsiColor::Yellow.on_default())
}

pub(crate) fn hyperlink(label: impl std::fmt::Display, url: impl std::fmt::Display) -> String {
    format!("\x1b]8;;{url}\x1b\\{label}\x1b]8;;\x1b\\")
}

pub(crate) fn table(rows: Vec<Vec<String>>) -> String {
    use tabled::{
        builder::Builder,
        settings::{Padding, Style},
    };

    let column_count = rows.iter().map(Vec::len).max().unwrap_or_default();
    let mut builder = Builder::with_capacity(rows.len(), column_count);
    for mut row in rows {
        row.resize(column_count, String::new());
        builder.push_record(row);
    }

    builder
        .build()
        .with(Style::empty())
        .with(Padding::new(0, 2, 0, 0))
        .to_string()
        .lines()
        .map(str::trim_end)
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    #[test]
    fn formats_error_prefixes_red() {
        assert_eq!(
            super::format_error_message("error: missing argument\n\nUsage: nitpick review start"),
            "\u{1b}[31merror:\u{1b}[0m missing argument\n\nUsage: nitpick review start"
        );
    }

    #[test]
    fn leaves_non_error_lines_unchanged() {
        assert_eq!(
            super::format_error_message("Usage: nitpick review start"),
            "Usage: nitpick review start"
        );
    }

    #[test]
    fn aligns_table_columns_ignoring_ansi_escape_codes() {
        assert_eq!(
            super::table(vec![
                vec![super::success("ok"), "short".into(), "1".into()],
                vec![super::error("error"), "longer".into(), "2".into()],
            ]),
            "\u{1b}[32mok\u{1b}[0m     short   1\n\u{1b}[31merror\u{1b}[0m  longer  2"
        );
    }
}

pub(crate) fn colorize(value: impl std::fmt::Display, style: anstyle::Style) -> String {
    format!("{}{}{}", style.render(), value, style.render_reset())
}

fn status_style(status: &ActivityStatus) -> anstyle::Style {
    match status {
        ActivityStatus::Queued => anstyle::AnsiColor::Cyan.on_default(),
        ActivityStatus::Running => anstyle::AnsiColor::Blue.on_default(),
        ActivityStatus::Completed => anstyle::AnsiColor::Green.on_default(),
        ActivityStatus::Error => anstyle::AnsiColor::Red.on_default(),
        ActivityStatus::Cancelled => anstyle::AnsiColor::Yellow.on_default(),
    }
}
