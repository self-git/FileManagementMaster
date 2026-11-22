use std::{
    env,
    ffi::OsString,
    fs,
    io::{BufRead, BufReader, Read, Write},
    path::{Path, PathBuf},
    process::{Child, Command, Output, Stdio},
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{bail, ensure, Context, Result};
use serde::{Deserialize, Serialize};

#[cfg(target_os = "macos")]
use std::os::unix::process::CommandExt;
#[cfg(windows)]
use std::os::windows::process::CommandExt;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Distribution {
    executable: String,
    exec_path: String,
    #[serde(default)]
    version_suffix: String,
}

#[derive(Deserialize)]
struct RuntimeConfig {
    version: String,
    macos: Distribution,
    windows: Distribution,
}

pub(crate) struct Git {
    runtime_dir: PathBuf,
    binary: PathBuf,
    exec_path: PathBuf,
    search_path: OsString,
    expected_version: String,
}

impl Git {
    pub(crate) fn bundled(runtime_dir: &Path) -> Result<Self> {
        ensure!(runtime_dir.is_absolute(), "随包 Git 目录必须为绝对路径");
        let config: RuntimeConfig =
            serde_json::from_str(include_str!("../../scripts/git-runtime.json"))?;
        let distribution = if cfg!(windows) {
            config.windows
        } else {
            config.macos
        };
        let binary = runtime_dir.join(distribution.executable);
        let exec_path = runtime_dir.join(distribution.exec_path);
        ensure!(binary.is_file(), "找不到随包 Git {}", binary.display());
        ensure!(
            exec_path.is_dir(),
            "找不到 Git 命令目录 {}",
            exec_path.display()
        );

        let bin_dir = binary.parent().context("随包 Git 缺少程序目录")?;
        let mut search_paths = vec![bin_dir.to_owned(), exec_path.clone()];
        if cfg!(windows) {
            search_paths.push(runtime_dir.join("usr/bin"));
        }
        Ok(Self {
            runtime_dir: runtime_dir.to_owned(),
            binary,
            exec_path,
            search_path: env::join_paths(search_paths)?,
            expected_version: format!(
                "git version {}{}",
                config.version, distribution.version_suffix
            ),
        })
    }

    pub(crate) fn binary(&self) -> &Path {
        &self.binary
    }

    pub(crate) fn version(&self, directory: &Path) -> Result<String> {
        let output = self.command(directory, &["--version"]).output()?;
        let version = stdout_text(output, "version")?;
        ensure!(
            version == self.expected_version,
            "随包 Git 版本为 {version}，预期 {}",
            self.expected_version
        );
        Ok(version)
    }

    fn command(&self, directory: &Path, args: &[&str]) -> Command {
        let mut command = Command::new(&self.binary);
        let empty_config = if cfg!(windows) { "NUL" } else { "/dev/null" };
        command
            .current_dir(directory)
            .env_clear()
            .env("PATH", &self.search_path)
            .env("GIT_EXEC_PATH", &self.exec_path)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", empty_config)
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("GIT_LITERAL_PATHSPECS", "1")
            .env("LC_ALL", "C")
            .args([
                "-c",
                "user.name=FileManagementMaster",
                "-c",
                "user.email=local@filemanagement.invalid",
                "-c",
                "commit.gpgsign=false",
                "-c",
                "core.logAllRefUpdates=false",
                "-c",
                "core.fsmonitor=false",
                "-c",
                "core.autocrlf=false",
                "-c",
                "core.filemode=false",
                "-c",
                "maintenance.auto=false",
                "-c",
                "gc.auto=0",
            ])
            .args(args);
        for key in ["SystemRoot", "WINDIR", "TEMP", "TMP", "TMPDIR"] {
            if let Some(value) = env::var_os(key) {
                command.env(key, value);
            }
        }
        #[cfg(windows)]
        command.creation_flags(windows_sys::Win32::System::Threading::CREATE_NO_WINDOW);
        command
    }
}

pub(crate) struct Repository<'a> {
    git: &'a Git,
    root: PathBuf,
}

impl<'a> Repository<'a> {
    pub(crate) fn create(git: &'a Git, root: PathBuf) -> Result<Self> {
        fs::create_dir(&root)?;
        let repository = Self::at(git, root);
        repository.initialize("reading")?;
        fs::create_dir(repository.control_dir())?;
        Ok(repository)
    }

    pub(crate) fn at(git: &'a Git, root: PathBuf) -> Self {
        Self { git, root }
    }

    pub(crate) fn initialize(&self, branch: &str) -> Result<()> {
        self.run(&[
            "init",
            "--quiet",
            "--template=",
            "--ref-format=reftable",
            "--initial-branch",
            branch,
        ])?;
        for (name, value) in [
            ("core.logAllRefUpdates", "false"),
            ("gc.auto", "0"),
            ("maintenance.auto", "false"),
        ] {
            self.run(&["config", "--local", name, value])?;
        }
        Ok(())
    }

    pub(crate) fn root(&self) -> &Path {
        &self.root
    }

    pub(crate) fn control_dir(&self) -> PathBuf {
        self.root.join(".git").join("filemanagement")
    }

    pub(crate) fn run(&self, args: &[&str]) -> Result<String> {
        stdout_text(self.exec(args, None, None)?, args[0])
    }

    pub(crate) fn data(&self, args: &[&str]) -> Result<Vec<u8>> {
        Ok(require_success(self.exec(args, None, None)?, args[0])?.stdout)
    }

    pub(crate) fn write_blob(&self, bytes: &[u8]) -> Result<String> {
        stdout_text(
            self.exec(&["hash-object", "-w", "--stdin"], Some(bytes), None)?,
            "hash-object",
        )
    }

    pub(crate) fn file_oid(&self, path: &Path, persist: bool) -> Result<String> {
        let args: &[&str] = if persist {
            &["hash-object", "-w", "--stdin"]
        } else {
            &["hash-object", "--stdin"]
        };
        let file = fs::File::open(path).with_context(|| format!("打开文件 {}", path.display()))?;
        let output = self.git.command(&self.root, args).stdin(file).output()?;
        stdout_text(output, "hash-object")
    }

    pub(crate) fn extract_blob(&self, oid: &str, file: &fs::File) -> Result<()> {
        let output = self
            .git
            .command(&self.root, &["cat-file", "blob", oid])
            .stdout(file.try_clone()?)
            .output()?;
        require_success(output, "cat-file")?;
        Ok(())
    }

    pub(crate) fn reference(&self, name: &str) -> Result<Option<String>> {
        let probe = self.exec(&["show-ref", "--exists", name], None, None)?;
        if probe.status.code() == Some(2) {
            return Ok(None);
        }
        require_success(probe, "show-ref")?;
        self.run(&["rev-parse", "--verify", name]).map(Some)
    }

    pub(crate) fn snapshot<'b>(
        &self,
        entries: impl IntoIterator<Item = (&'b str, &'b str)>,
        message: &str,
    ) -> Result<String> {
        self.snapshot_modes(
            entries.into_iter().map(|(path, oid)| (path, oid, false)),
            message,
        )
    }

    pub(crate) fn snapshot_modes<'b>(
        &self,
        entries: impl IntoIterator<Item = (&'b str, &'b str, bool)>,
        message: &str,
    ) -> Result<String> {
        let scratch = tempfile::tempdir_in(self.control_dir())?;
        let index = scratch.path().join("index");
        require_success(
            self.exec(&["read-tree", "--empty"], None, Some(&index))?,
            "read-tree",
        )?;

        // NUL 分隔的输入保留文件名里的空格和中文，隔离索引只用于装配快照。
        let mut changes = Vec::new();
        for (path, oid, executable) in entries {
            let mode = if executable { "100755" } else { "100644" };
            write!(&mut changes, "{mode} {oid}\t{path}\0")?;
        }
        if !changes.is_empty() {
            require_success(
                self.exec(
                    &["update-index", "-z", "--index-info"],
                    Some(&changes),
                    Some(&index),
                )?,
                "update-index",
            )?;
        }
        let tree = stdout_text(
            self.exec(&["write-tree"], None, Some(&index))?,
            "write-tree",
        )?;
        self.run(&["commit-tree", &tree, "-m", message])
    }

    pub(crate) fn object_exists(&self, oid: &str) -> Result<bool> {
        Ok(self
            .exec(&["cat-file", "-e", oid], None, None)?
            .status
            .success())
    }

    pub(crate) fn write_worktree_index(&self, commit: &str) -> Result<()> {
        // 先写隔离索引。中断时实际 index 仍完整，重开可按已发布的引用重做。
        let scratch = tempfile::tempdir_in(self.control_dir())?;
        let index = scratch.path().join("index");
        require_success(
            self.exec(&["read-tree", commit], None, Some(&index))?,
            "read-tree",
        )?;
        fs::OpenOptions::new()
            .write(true)
            .open(&index)?
            .sync_all()?;
        fs::rename(index, self.root.join(".git/index")).context("更新工作目录索引")
    }

    pub(crate) fn update_references(&self, edits: &[RefEdit]) -> Result<()> {
        self.publish(&edit_lines(edits))
    }

    pub(crate) fn switch_symbolic_head(
        &self,
        previous: &str,
        next: &str,
        catalog: &RefEdit,
    ) -> Result<()> {
        let edits = format!(
            "option no-deref\nsymref-update HEAD {next} ref {previous}\n{}",
            edit_lines(std::slice::from_ref(catalog))
        );
        self.publish(&edits)
    }

    pub(crate) fn start_ref_update(&self) -> Result<RunningGit> {
        let mut command = self.git.command(&self.root, &["update-ref", "--stdin"]);
        command.stdin(Stdio::piped());
        RunningGit::spawn(command)
    }

    pub(crate) fn start_gc(&self) -> Result<RunningGit> {
        RunningGit::spawn(
            self.git
                .command(&self.root, &["gc", "--prune=now", "--no-detach"]),
        )
    }

    pub(crate) fn start_recorded_gc(&self, worker: &Path) -> Result<RunningGit> {
        let mut command = Command::new(worker);
        command
            .arg("--filemanagement-gc")
            .arg(&self.git.runtime_dir)
            .arg(&self.root);
        command.stdin(Stdio::piped());
        #[cfg(windows)]
        command.creation_flags(windows_sys::Win32::System::Threading::CREATE_NO_WINDOW);
        RunningGit::spawn(command)
    }

    pub(crate) fn object_bytes(&self) -> Result<u64> {
        file_bytes(&self.root.join(".git/objects"))
    }

    fn publish(&self, edits: &str) -> Result<()> {
        let batch = format!("start\n{edits}prepare\ncommit\n");
        require_success(
            self.exec(&["update-ref", "--stdin"], Some(batch.as_bytes()), None)?,
            "update-ref",
        )?;
        Ok(())
    }

    fn exec(&self, args: &[&str], input: Option<&[u8]>, index: Option<&Path>) -> Result<Output> {
        let mut command = self.git.command(&self.root, args);
        if let Some(index) = index {
            command.env("GIT_INDEX_FILE", index);
        }
        let Some(input) = input else {
            return command
                .output()
                .with_context(|| format!("无法执行 Git {}", args[0]));
        };

        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        let mut stdin = child.stdin.take().context("缺少 Git 标准输入")?;
        std::thread::scope(|scope| {
            let writer = scope.spawn(move || stdin.write_all(input));
            let output = child.wait_with_output()?;
            let sent = writer
                .join()
                .map_err(|_| anyhow::anyhow!("Git 输入线程异常退出"))?;
            if output.status.success() {
                sent?;
            }
            Ok(output)
        })
    }
}

fn file_bytes(directory: &Path) -> Result<u64> {
    fs::read_dir(directory)?.try_fold(0, |total, item| {
        let item = item?;
        let info = item.metadata()?;
        let size = if info.is_dir() {
            file_bytes(&item.path())?
        } else {
            info.len()
        };
        Ok(total + size)
    })
}

pub fn run_gc_worker(runtime: &Path, directory: &Path) -> Result<()> {
    // 父进程先记录写进程身份，收到许可前本进程不接触对象库。
    let mut signal = [0];
    std::io::stdin().read_exact(&mut signal)?;
    ensure!(signal == *b"g", "回收进程没有启动凭证");
    let git = Git::bundled(runtime)?;
    let repo = Repository::at(&git, directory.to_owned());
    ensure!(crate::worktree::owned_library(&repo)?, "回收目录未纳管");
    let stamp: ProcessStamp =
        serde_json::from_reader(fs::File::open(repo.control_dir().join("gc.writer"))?)?;
    ensure!(stamp.pid == std::process::id(), "回收进程与书库记录不一致");
    ensure!(
        crate::worktree::WriteLease::try_acquire(&repo)?.is_none(),
        "回收未持有书库写锁"
    );
    let status = git
        .command(
            directory,
            &[
                "-c",
                "gc.packRefs=false",
                "gc",
                "--prune=now",
                "--no-detach",
            ],
        )
        .stdin(Stdio::null())
        .status()?;
    ensure!(status.success(), "Git 回收退出状态 {status}");
    Ok(())
}

#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct RefEdit {
    pub(crate) name: String,
    pub(crate) old: Option<String>,
    pub(crate) new: String,
}

impl RefEdit {
    pub(crate) fn create(name: &str, new: &str) -> Self {
        Self {
            name: name.to_owned(),
            old: None,
            new: new.to_owned(),
        }
    }

    pub(crate) fn replace(name: &str, old: &str, new: &str) -> Self {
        Self {
            name: name.to_owned(),
            old: Some(old.to_owned()),
            new: new.to_owned(),
        }
    }
}

fn edit_lines(edits: &[RefEdit]) -> String {
    let mut batch = String::new();
    for edit in edits {
        match &edit.old {
            Some(old) => batch.push_str(&format!("update {} {} {old}\n", edit.name, edit.new)),
            None => batch.push_str(&format!("create {} {}\n", edit.name, edit.new)),
        }
    }
    batch
}

fn require_success(output: Output, name: &str) -> Result<Output> {
    if !output.status.success() {
        bail!(
            "Git {name} 执行失败（{}）\n{}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(output)
}

fn stdout_text(output: Output, name: &str) -> Result<String> {
    Ok(String::from_utf8(require_success(output, name)?.stdout)?
        .trim()
        .to_owned())
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ProcessStamp {
    pid: u32,
    registered_at_ms: u64,
}

impl ProcessStamp {
    pub(crate) fn is_running(self) -> Result<bool> {
        ensure!(
            self.pid > 0 && self.pid <= i32::MAX as u32,
            "无效的 Git 进程编号"
        );
        #[cfg(target_os = "macos")]
        {
            // 检查整个子进程组。PID 已被复用时也保守地等待。
            if unsafe { libc::kill(-(self.pid as i32), 0) } == 0 {
                return Ok(true);
            }
            let error = std::io::Error::last_os_error();
            match error.raw_os_error() {
                Some(libc::ESRCH) => Ok(false),
                Some(libc::EPERM) => Ok(true),
                _ => Err(error).context("检查 Git 进程组"),
            }
        }
        #[cfg(windows)]
        {
            use windows_sys::Win32::{
                Foundation::{CloseHandle, ERROR_INVALID_PARAMETER, STILL_ACTIVE},
                System::Threading::{
                    GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
                },
            };
            let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, self.pid) };
            if process.is_null() {
                let error = std::io::Error::last_os_error();
                return match error.raw_os_error() {
                    Some(code) if code == ERROR_INVALID_PARAMETER as i32 => Ok(false),
                    _ => Err(error).context("检查 Git 进程"),
                };
            }
            let mut exit_code = 0;
            let status = unsafe { GetExitCodeProcess(process, &mut exit_code) };
            let error = std::io::Error::last_os_error();
            unsafe { CloseHandle(process) };
            ensure!(status != 0, "读取 Git 进程状态 {error}");
            Ok(exit_code == STILL_ACTIVE as u32)
        }
        #[cfg(not(any(target_os = "macos", windows)))]
        bail!("当前平台不支持检查 Git 写进程");
    }

    pub(crate) fn predates(self, lock: &fs::Metadata) -> Result<bool> {
        let modified = lock.modified()?.duration_since(UNIX_EPOCH)?;
        Ok(modified.as_secs() >= self.registered_at_ms / 1000)
    }
}

pub(crate) struct ExitedProcess(ProcessStamp);

impl ExitedProcess {
    pub(crate) fn stamp(&self) -> ProcessStamp {
        self.0
    }
}

pub(crate) struct RunningGit {
    child: Option<Child>,
    stamp: ProcessStamp,
    #[cfg(windows)]
    job: windows_job::Job,
}

impl RunningGit {
    fn spawn(mut command: Command) -> Result<Self> {
        #[cfg(target_os = "macos")]
        command.process_group(0);
        #[cfg(windows)]
        let job = windows_job::Job::create()?;
        let mut child = command
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        #[cfg(windows)]
        if let Err(error) = job.attach(&child) {
            let _ = child.kill();
            let _ = child.wait();
            return Err(error);
        }
        #[cfg(not(windows))]
        let _ = &mut child;

        let stamp = ProcessStamp {
            pid: child.id(),
            registered_at_ms: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64,
        };
        Ok(Self {
            child: Some(child),
            stamp,
            #[cfg(windows)]
            job,
        })
    }

    pub(crate) fn stamp(&self) -> ProcessStamp {
        self.stamp
    }

    pub(crate) fn prepare_references(&mut self, edits: &[RefEdit]) -> Result<()> {
        self.prepare(&edit_lines(edits))
    }

    pub(crate) fn prepare_switch(
        &mut self,
        previous: &str,
        next: &str,
        new_commit: &str,
        edits: &[RefEdit],
    ) -> Result<()> {
        self.prepare(&format!(
            "verify {next} {new_commit}\noption no-deref\nsymref-update HEAD {next} ref {previous}\n{}",
            edit_lines(edits)
        ))
    }

    fn prepare(&mut self, edits: &str) -> Result<()> {
        let child = self.child.as_mut().context("Git 写进程已结束")?;
        let batch = format!("start\n{edits}prepare\n");
        let stdin = child.stdin.as_mut().context("缺少引用事务输入")?;
        stdin.write_all(batch.as_bytes())?;
        stdin.flush()?;

        let mut stdout = BufReader::new(child.stdout.as_mut().context("缺少引用事务输出")?);
        for expected in ["start: ok", "prepare: ok"] {
            let mut line = String::new();
            stdout.read_line(&mut line)?;
            if line.trim_end() != expected {
                drop(stdout);
                let mut stderr = String::new();
                if line.is_empty() {
                    child
                        .stderr
                        .as_mut()
                        .context("缺少 Git 错误输出")?
                        .read_to_string(&mut stderr)?;
                }
                bail!("Git 引用事务未返回 {expected} {}", stderr.trim());
            }
        }
        Ok(())
    }

    pub(crate) fn commit_prepared(mut self) -> Result<()> {
        let child = self.child.as_mut().context("Git 写进程已结束")?;
        child
            .stdin
            .take()
            .context("缺少引用事务输入")?
            .write_all(b"commit\n")?;
        let child = self.child.take().context("Git 写进程已结束")?;
        require_success(child.wait_with_output()?, "update-ref")?;
        Ok(())
    }

    pub(crate) fn interrupt(mut self) -> Result<ExitedProcess> {
        self.terminate()?;
        self.child.take().context("Git 写进程已结束")?.wait()?;
        Ok(ExitedProcess(self.stamp))
    }

    pub(crate) fn finish_gc(mut self) -> Result<()> {
        let child = self.child.take().context("Git 回收进程已结束")?;
        require_success(child.wait_with_output()?, "gc")?;
        Ok(())
    }

    pub(crate) fn release_gc_worker(&mut self) -> Result<()> {
        self.child
            .as_mut()
            .and_then(|child| child.stdin.take())
            .context("缺少回收进程输入")?
            .write_all(b"g")?;
        Ok(())
    }

    fn terminate(&mut self) -> Result<()> {
        let Some(child) = self.child.as_mut() else {
            return Ok(());
        };
        if child.try_wait()?.is_some() {
            return Ok(());
        }
        #[cfg(target_os = "macos")]
        {
            // 仅终止此执行器刚创建且尚未回收的 Git 进程组。
            let result = unsafe { libc::kill(-(child.id() as i32), libc::SIGKILL) };
            if result != 0 {
                return Err(std::io::Error::last_os_error().into());
            }
        }
        #[cfg(windows)]
        self.job.terminate()?;
        #[cfg(not(any(target_os = "macos", windows)))]
        child.kill()?;
        Ok(())
    }
}

impl Drop for RunningGit {
    fn drop(&mut self) {
        let _ = self.terminate();
        if let Some(child) = self.child.as_mut() {
            let _ = child.wait();
        }
    }
}

#[cfg(windows)]
mod windows_job {
    use std::{io, mem, os::windows::io::AsRawHandle, process::Child, ptr};

    use anyhow::{Context, Result};
    use windows_sys::Win32::{
        Foundation::{CloseHandle, HANDLE},
        System::JobObjects::{
            AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
            SetInformationJobObject, TerminateJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
            JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        },
    };

    pub(super) struct Job(HANDLE);

    impl Job {
        pub(super) fn create() -> Result<Self> {
            let handle = unsafe { CreateJobObjectW(ptr::null(), ptr::null()) };
            if handle.is_null() {
                return Err(io::Error::last_os_error()).context("创建 Git Job Object");
            }
            let job = Self(handle);
            let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { mem::zeroed() };
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            let result = unsafe {
                SetInformationJobObject(
                    job.0,
                    JobObjectExtendedLimitInformation,
                    (&info as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
                    mem::size_of_val(&info) as u32,
                )
            };
            if result == 0 {
                return Err(io::Error::last_os_error()).context("配置 Git Job Object");
            }
            Ok(job)
        }

        pub(super) fn attach(&self, child: &Child) -> Result<()> {
            if unsafe { AssignProcessToJobObject(self.0, child.as_raw_handle().cast()) } == 0 {
                return Err(io::Error::last_os_error()).context("将 Git 写进程加入 Job Object");
            }
            Ok(())
        }

        pub(super) fn terminate(&self) -> Result<()> {
            if unsafe { TerminateJobObject(self.0, 1) } == 0 {
                return Err(io::Error::last_os_error()).context("终止已启动的 Git 进程组");
            }
            Ok(())
        }
    }

    impl Drop for Job {
        fn drop(&mut self) {
            unsafe { CloseHandle(self.0) };
        }
    }
}
