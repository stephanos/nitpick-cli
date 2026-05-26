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
        self.rules.push("(allow process-exec)".into());
        self.rules.push("(allow process-fork)".into());
        self.rules
            .push("(allow process-info* (target same-sandbox))".into());
        self.rules
            .push("(allow signal (target same-sandbox))".into());
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
        self.rules.push(
            r#"(allow sysctl-read
 (sysctl-name "hw.activecpu")
 (sysctl-name "hw.busfrequency_compat")
 (sysctl-name "hw.byteorder")
 (sysctl-name "hw.cacheconfig")
 (sysctl-name "hw.cachelinesize_compat")
 (sysctl-name "hw.cpufamily")
 (sysctl-name "hw.cpufrequency")
 (sysctl-name "hw.cpufrequency_compat")
 (sysctl-name "hw.cputype")
 (sysctl-name "hw.l1dcachesize_compat")
 (sysctl-name "hw.l1icachesize_compat")
 (sysctl-name "hw.l2cachesize_compat")
 (sysctl-name "hw.l3cachesize_compat")
 (sysctl-name "hw.logicalcpu")
 (sysctl-name "hw.logicalcpu_max")
 (sysctl-name "hw.machine")
 (sysctl-name "hw.memsize")
 (sysctl-name "hw.model")
 (sysctl-name "hw.ncpu")
 (sysctl-name "hw.nperflevels")
 (sysctl-name "hw.packages")
 (sysctl-name "hw.pagesize")
 (sysctl-name "hw.pagesize_compat")
 (sysctl-name "hw.physicalcpu")
 (sysctl-name "hw.physicalcpu_max")
 (sysctl-name "hw.tbfrequency_compat")
 (sysctl-name "hw.vectorunit")
 (sysctl-name "kern.argmax")
 (sysctl-name "kern.hostname")
 (sysctl-name "kern.maxfiles")
 (sysctl-name "kern.maxfilesperproc")
 (sysctl-name "kern.maxproc")
 (sysctl-name "kern.ngroups")
 (sysctl-name "kern.osproductversion")
 (sysctl-name "kern.osrelease")
 (sysctl-name "kern.ostype")
 (sysctl-name "kern.osvariant_status")
 (sysctl-name "kern.osversion")
 (sysctl-name "kern.secure_kernel")
 (sysctl-name "kern.tcsm_available")
 (sysctl-name "kern.tcsm_enable")
 (sysctl-name "kern.usrstack64")
 (sysctl-name "kern.version")
 (sysctl-name "machdep.cpu.brand_string")
 (sysctl-name "machdep.ptrauth_enabled")
 (sysctl-name "security.mac.lockdown_mode_state")
 (sysctl-name "sysctl.proc_cputype")
 (sysctl-name "vm.loadavg")
 (sysctl-name-prefix "hw.optional.arm")
 (sysctl-name-prefix "hw.optional.arm.")
 (sysctl-name-prefix "hw.optional.armv8_")
 (sysctl-name-prefix "hw.perflevel")
 (sysctl-name-prefix "kern.proc.all")
 (sysctl-name-prefix "kern.proc.pgrp.")
 (sysctl-name-prefix "kern.proc.pid.")
 (sysctl-name-prefix "machdep.cpu.")
 (sysctl-name-prefix "net.routetable.")
)"#
            .into(),
        );
        self.rules
            .push(r#"(allow sysctl-write (sysctl-name "kern.tcsm_enable"))"#.into());
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
        self.rules
            .push("(allow ipc-posix-shm-read* ipc-posix-shm-write*)".into());
        self.rules.push("(allow ipc-posix-sem)".into());
        self.rules
            .push(r#"(allow ipc-posix-shm-read* (ipc-posix-name-prefix "apple.cfprefs."))"#.into());
        self.rules.push("(allow pseudo-tty)".into());
        self.rules
            .push("(allow distributed-notification-post)".into());
        self.rules.push(r#"(allow user-preference-read)"#.into());
        self.rules.push(
            r#"(allow mach-lookup
 (global-name "com.apple.SecurityServer")
 (global-name "com.apple.analyticsd")
 (global-name "com.apple.analyticsd.messagetracer")
 (global-name "com.apple.appsleep")
 (global-name "com.apple.audio.AudioComponentRegistrar")
 (global-name "com.apple.audio.audiohald")
 (global-name "com.apple.audio.systemsoundserver")
 (global-name "com.apple.bsd.dirhelper")
 (global-name "com.apple.cfprefsd.agent")
 (global-name "com.apple.cfprefsd.daemon")
 (global-name "com.apple.coreservices.launchservicesd")
 (global-name "com.apple.diagnosticd")
 (global-name "com.apple.distributed_notifications@Uv3")
 (global-name "com.apple.FontObjectsServer")
 (global-name "com.apple.fonts")
 (global-name "com.apple.logd")
 (global-name "com.apple.logd.events")
 (global-name "com.apple.lsd.mapdb")
 (global-name "com.apple.networkd")
 (global-name "com.apple.ocspd")
 (global-name "com.apple.PowerManagement.control")
 (global-name "com.apple.securityd.xpc")
 (global-name "com.apple.system.DirectoryService.libinfo_v1")
 (global-name "com.apple.system.logger")
 (global-name "com.apple.system.notification_center")
 (global-name "com.apple.system.opendirectoryd.libinfo")
 (global-name "com.apple.system.opendirectoryd.membership")
 (global-name "com.apple.SystemConfiguration.configd")
 (global-name "com.apple.SystemConfiguration.DNSConfiguration")
 (global-name "com.apple.trustd")
 (global-name "com.apple.trustd.agent")
 (global-name "com.apple.xpc.activity.unmanaged")
 (local-name "com.apple.cfprefsd.agent")
)"#
            .into(),
        );
        self.rules.push(
            r#"(allow iokit-open
 (iokit-registry-entry-class "IOSurfaceRootUserClient")
 (iokit-registry-entry-class "RootDomainUserClient")
 (iokit-user-client-class "IOSurfaceSendRight")
)"#
            .into(),
        );
        self.rules.push("(allow iokit-get-properties)".into());
        self.rules.push(
            r#"(allow system-socket (require-all (socket-domain AF_SYSTEM) (socket-protocol 2)))"#
                .into(),
        );
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

    pub(crate) fn allow_regex_read_writes(mut self, patterns: &[String]) -> Self {
        for pattern in patterns {
            self = self.allow_regex_read_write(pattern);
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

    pub(crate) fn allow_regex_read_write(mut self, pattern: &str) -> Self {
        self.rules
            .push(sandbox_regex_rule("file-read* file-write*", pattern));
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

fn sandbox_regex_rule(operation: &str, pattern: &str) -> String {
    let pattern = escape_sandbox_string(pattern);
    format!(r#"(allow {operation} (regex "{pattern}"))"#)
}

pub(crate) fn regex_literal_path(path: &Path) -> String {
    let path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    escape_regex_literal(&path.to_string_lossy())
}

fn escape_regex_literal(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for character in value.chars() {
        if matches!(
            character,
            '\\' | '.' | '+' | '*' | '?' | '(' | ')' | '|' | '[' | ']' | '{' | '}' | '^' | '$'
        ) {
            escaped.push('\\');
        }
        escaped.push(character);
    }
    escaped
}

fn escape_sandbox_string(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}
