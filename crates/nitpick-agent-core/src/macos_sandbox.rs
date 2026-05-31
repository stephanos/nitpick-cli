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
        self.rules.push("(allow signal)".into());
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
        self.rules.push("(allow sysctl*)".into());
        self
    }

    pub(crate) fn allow_file_read_metadata(mut self) -> Self {
        self.rules.push("(allow file-read-metadata)".into());
        self.rules.push("(allow file-test-existence)".into());
        self
    }

    pub(crate) fn allow_device_runtime(mut self) -> Self {
        self.rules
            .push(r#"(allow file-read* file-write* (literal "/dev/null"))"#.into());
        self.rules
            .push(r#"(allow file-read* file-write* (literal "/dev/zero"))"#.into());
        self.rules
            .push(r#"(allow file-read* (literal "/dev/random") (literal "/dev/urandom"))"#.into());
        self.rules.push(
            r#"(allow file-read-data file-test-existence file-write-data (subpath "/dev/fd"))"#
                .into(),
        );
        self.rules
            .push(r#"(allow file-read* (regex "^/dev/fd/(0|1|2)$"))"#.into());
        self.rules
            .push(r#"(allow file-write* (regex "^/dev/fd/(1|2)$"))"#.into());
        self.rules
            .push(r#"(allow file-read* file-write* (literal "/dev/tty"))"#.into());
        self.rules
            .push(r#"(allow file-read* file-write* file-ioctl (literal "/dev/ptmx"))"#.into());
        self.rules
            .push(r#"(allow file-read* file-write* (regex "^/dev/ttys[0-9]+$"))"#.into());
        self.rules
            .push(r#"(allow file-ioctl (literal "/dev/null") (literal "/dev/zero") (literal "/dev/random") (literal "/dev/urandom") (literal "/dev/tty") (regex "^/dev/ttys[0-9]+$"))"#.into());
        self
    }

    pub(crate) fn allow_macos_runtime(mut self) -> Self {
        for path in [
            Path::new("/System"),
            Path::new("/Library"),
            Path::new("/private/etc"),
            Path::new("/private/var/db/timezone"),
            Path::new("/etc"),
            Path::new("/usr"),
            Path::new("/bin"),
            Path::new("/sbin"),
            Path::new("/Applications"),
        ] {
            self = self.allow_read(path);
        }
        self.rules.push(
            r#"(allow file-map-executable
 (subpath "/Library/Apple/System/Library/Frameworks")
 (subpath "/Library/Apple/System/Library/PrivateFrameworks")
 (subpath "/Library/Apple/usr/lib")
 (subpath "/System/Library/Extensions")
 (subpath "/System/Library/Frameworks")
 (subpath "/System/Library/PrivateFrameworks")
 (subpath "/System/Library/SubFrameworks")
 (subpath "/System/iOSSupport/System/Library/Frameworks")
 (subpath "/System/iOSSupport/System/Library/PrivateFrameworks")
 (subpath "/System/iOSSupport/System/Library/SubFrameworks")
 (subpath "/usr/lib")
)"#
            .into(),
        );
        self.rules
            .push(r#"(allow file-read* file-test-existence (literal "/"))"#.into());
        self.rules.push(
            r#"(allow file-read-metadata file-test-existence
 (literal "/etc")
 (literal "/tmp")
 (literal "/var")
 (literal "/private/etc/localtime")
 (path-ancestors "/System/Volumes/Data/private")
 (subpath "/var")
 (subpath "/private/var")
)"#
            .into(),
        );
        self.rules.push(
            r#"(allow file-read* file-test-existence
 (literal "/System/Library/CoreServices")
 (literal "/System/Library/CoreServices/.SystemVersionPlatform.plist")
 (literal "/System/Library/CoreServices/SystemVersion.plist")
 (literal "/dev/autofs_nowait")
 (literal "/private/etc/master.passwd")
 (literal "/private/etc/passwd")
 (literal "/private/etc/protocols")
 (literal "/private/etc/services")
 (literal "/private/var/db/eligibilityd/eligibility.plist")
)"#
            .into(),
        );
        self.rules
            .push(r#"(allow system-mac-syscall (mac-policy-name "vnguard"))"#.into());
        self.rules.push(
            r#"(allow system-mac-syscall (require-all (mac-policy-name "Sandbox") (mac-syscall-number 67)))"#.into(),
        );
        self.rules
            .push(r#"(allow system-fsctl (fsctl-command FSIOC_CAS_BSDFLAGS))"#.into());
        self.rules.push("(allow ipc*)".into());
        self.rules.push("(allow pseudo-tty)".into());
        self.rules
            .push("(allow distributed-notification-post)".into());
        self.rules.push(r#"(allow user-preference-read)"#.into());
        self.rules.push("(allow iokit*)".into());
        self.rules.push("(allow system*)".into());
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

    pub(crate) fn allow_literal_read_writes(mut self, paths: &[PathBuf]) -> Self {
        for path in paths {
            self = self.allow_literal_read_write(path);
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

    pub(crate) fn allow_literal_read_write(mut self, path: &Path) -> Self {
        self.rules
            .push(sandbox_literal_rule("file-read* file-write*", path));
        self
    }

    pub(crate) fn render(self) -> String {
        let mut profile = "(version 1)\n(deny default)\n".to_owned();
        profile.push_str(&self.rules.join("\n"));
        profile.push('\n');
        profile
    }

    pub(crate) fn render_with_deny_message(self, message: &str) -> String {
        let message = escape_sandbox_string(message);
        let mut profile = format!("(version 1)\n(deny default (with message \"{message}\"))\n");
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
