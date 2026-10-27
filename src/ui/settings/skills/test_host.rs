//! Real local filesystem with an explicit non-secret Host environment for tests.
use super::*;
use lyrel_core::core::agent_history::PastSession;
use lyrel_core::host::{
    ContentLimits, ContentQuery, ContentResults, Host, Meta, Output, SearchHit, SharedHost,
    WatchSub,
    session_operations::{SessionOperation, SessionOperationError},
};
struct IsolatedHost {
    inner: SharedHost,
    home: PathBuf,
    id: HostId,
    relative_state: bool,
}
pub(crate) fn install(home: &Path, cx: &mut Context<SettingsWindow>) {
    install_on(home, HostId::LOCAL, false, cx);
}
pub(super) fn install_on(
    home: &Path,
    id: HostId,
    relative_state: bool,
    cx: &mut Context<SettingsWindow>,
) {
    HostRegistry::insert(
        cx,
        Arc::new(IsolatedHost {
            inner: lyrel_core::host::local::LocalHost::new(),
            home: home.into(),
            id,
            relative_state,
        }),
    );
}
impl Host for IsolatedHost {
    fn id(&self) -> HostId {
        self.id
    }
    fn separator(&self) -> char {
        self.inner.separator()
    }
    fn is_absolute(&self, p: &Path) -> bool {
        self.inner.is_absolute(p)
    }
    fn read_dir(&self, p: &Path, root: Option<&Path>) -> io::Result<Vec<Entry>> {
        self.inner.read_dir(p, root)
    }
    fn stat(&self, p: &Path) -> io::Result<Meta> {
        self.inner.stat(p)
    }
    fn read_file(&self, p: &Path, max: u64) -> io::Result<Vec<u8>> {
        self.inner.read_file(p, max)
    }
    fn read_link(&self, p: &Path) -> io::Result<PathBuf> {
        self.inner.read_link(p)
    }
    fn canonicalize(&self, p: &Path) -> io::Result<PathBuf> {
        if self.relative_state && p == Path::new(".") {
            self.inner.canonicalize(&self.home.join("cwd"))
        } else {
            self.inner.canonicalize(p)
        }
    }
    fn search(
        &self,
        roots: &[PathBuf],
        query: &str,
        limit: usize,
        max_dirs: usize,
        hidden: bool,
    ) -> io::Result<Vec<SearchHit>> {
        self.inner.search(roots, query, limit, max_dirs, hidden)
    }
    fn search_content(
        &self,
        roots: &[PathBuf],
        query: &ContentQuery,
        limits: &ContentLimits,
    ) -> io::Result<ContentResults> {
        self.inner.search_content(roots, query, limits)
    }
    fn write_file(&self, p: &Path, bytes: &[u8]) -> io::Result<Meta> {
        self.inner.write_file(p, bytes)
    }
    fn create_file_new(&self, p: &Path) -> io::Result<()> {
        self.inner.create_file_new(p)
    }
    fn create_dir(&self, p: &Path, recursive: bool) -> io::Result<()> {
        self.inner.create_dir(p, recursive)
    }
    fn rename(&self, from: &Path, to: &Path) -> io::Result<()> {
        self.inner.rename(from, to)
    }
    fn remove(&self, p: &Path, recursive: bool) -> io::Result<()> {
        self.inner.remove(p, recursive)
    }
    fn repo_root(&self, p: &Path) -> io::Result<Option<PathBuf>> {
        self.inner.repo_root(p)
    }
    fn git(&self, p: &Path, args: &[&str]) -> io::Result<Output> {
        self.inner.git(p, args)
    }
    fn shells(&self) -> io::Result<lyrel_core::core::shells::ShellInventory> {
        self.inner.shells()
    }
    fn watch(&self, p: &[PathBuf]) -> io::Result<WatchSub> {
        self.inner.watch(p)
    }
    fn opencode_sessions(
        &self,
        _: &SessionOperation,
    ) -> Result<Vec<PastSession>, SessionOperationError> {
        Ok(Vec::new())
    }
    fn hermes_sessions(
        &self,
        _: &SessionOperation,
    ) -> Result<Vec<PastSession>, SessionOperationError> {
        Ok(Vec::new())
    }
    fn agent_environment(&self) -> io::Result<std::collections::BTreeMap<String, String>> {
        let h = self.home.to_string_lossy();
        Ok([
            ("HOME", h.to_string()),
            ("USERPROFILE", h.to_string()),
            (
                "LYREL_HOST_OS",
                if cfg!(windows) { "windows" } else { "linux" }.into(),
            ),
            ("XDG_CONFIG_HOME", format!("{h}/.config")),
            ("XDG_DATA_HOME", format!("{h}/.data")),
            (
                "XDG_STATE_HOME",
                if self.relative_state {
                    ".state".into()
                } else {
                    format!("{h}/.state")
                },
            ),
        ]
        .into_iter()
        .map(|(k, v)| (k.into(), v))
        .collect())
    }
}
