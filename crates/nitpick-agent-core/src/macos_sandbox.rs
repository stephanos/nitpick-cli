use std::path::{Path, PathBuf};

#[derive(Default)]
pub(crate) struct SandboxProfileBuilder {
    rules: Vec<String>,
}

impl SandboxProfileBuilder {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn allow_processes(mut self) -> Self {
        self.rules.push("(allow process*)".into());
        self
    }

    pub(crate) fn allow_mach_lookup(mut self) -> Self {
        self.rules.push("(allow mach-lookup)".into());
        self
    }

    pub(crate) fn allow_network(mut self) -> Self {
        self.rules.push("(allow network*)".into());
        self
    }

    pub(crate) fn allow_sysctl_read(mut self) -> Self {
        self.rules.push("(allow sysctl-read)".into());
        self
    }

    pub(crate) fn allow_file_read_metadata(mut self) -> Self {
        self.rules.push("(allow file-read-metadata)".into());
        self
    }

    pub(crate) fn allow_device_runtime(mut self) -> Self {
        self.rules
            .push(r#"(allow file-read* file-write* (literal "/dev/null"))"#.into());
        self.rules
            .push(r#"(allow file-read* (literal "/dev/random") (literal "/dev/urandom"))"#.into());
        self
    }

    pub(crate) fn allow_macos_runtime(mut self) -> Self {
        for path in [
            Path::new("/System"),
            Path::new("/Library"),
            Path::new("/private/etc"),
            Path::new("/etc"),
            Path::new("/usr"),
            Path::new("/bin"),
            Path::new("/sbin"),
        ] {
            self = self.allow_read(path);
        }
        self
    }

    pub(crate) fn allow_literal_reads(mut self, paths: &[PathBuf]) -> Self {
        for path in paths {
            self = self.allow_literal_read(path);
        }
        self
    }

    pub(crate) fn allow_reads(mut self, paths: &[PathBuf]) -> Self {
        for path in paths {
            self = self.allow_read(path);
        }
        self
    }

    pub(crate) fn allow_read_writes(mut self, paths: &[PathBuf]) -> Self {
        for path in paths {
            self = self.allow_read_write(path);
        }
        self
    }

    pub(crate) fn allow_literal_read(mut self, path: &Path) -> Self {
        self.rules.push(sandbox_literal_rule("file-read*", path));
        self
    }

    pub(crate) fn allow_read(mut self, path: &Path) -> Self {
        self.rules.push(sandbox_subpath_rule("file-read*", path));
        self
    }

    pub(crate) fn allow_read_write(mut self, path: &Path) -> Self {
        self.rules
            .push(sandbox_subpath_rule("file-read* file-write*", path));
        self
    }

    pub(crate) fn render(self) -> String {
        let mut profile = "(version 1)\n(deny default)\n".to_owned();
        profile.push_str(&self.rules.join("\n"));
        profile.push('\n');
        profile
    }
}

fn sandbox_literal_rule(operation: &str, path: &Path) -> String {
    let path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let path = escape_sandbox_string(&path.to_string_lossy());
    format!(r#"(allow {operation} (literal "{path}"))"#)
}

fn sandbox_subpath_rule(operation: &str, path: &Path) -> String {
    let path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let path = escape_sandbox_string(&path.to_string_lossy());
    format!(r#"(allow {operation} (subpath "{path}"))"#)
}

fn escape_sandbox_string(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}
