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

pub(crate) fn success(value: impl std::fmt::Display) -> String {
    colorize(value, anstyle::AnsiColor::Green.on_default())
}

pub(crate) fn error(value: impl std::fmt::Display) -> String {
    colorize(value, anstyle::AnsiColor::Red.on_default())
}

pub(crate) fn warn(value: impl std::fmt::Display) -> String {
    colorize(value, anstyle::AnsiColor::Yellow.on_default())
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
