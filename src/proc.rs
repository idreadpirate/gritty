// Detect the foreground process inside a pane by walking the shell's process
// tree. The tree logic is pure and tested; the OS snapshot is Windows-only.

use std::collections::HashMap;

pub struct Proc {
    pub pid: u32,
    pub ppid: u32,
    pub name: String,
}

/// A snapshot of the process list with a prebuilt parent→children map.
/// Build once per poll cycle and reuse across all panes (CA-3, RT-14, CA-38).
pub struct Snapshot {
    procs: Vec<Proc>,
    children: HashMap<u32, Vec<u32>>,
}

impl Snapshot {
    /// Capture the current process list and build the children map once.
    pub fn capture() -> Self {
        let procs = snapshot();
        let children = build_children_map(&procs);
        Self { procs, children }
    }

    /// Name of the foreground process in the tree rooted at `root_pid`, if any.
    /// Reuses the prebuilt children map — O(N) per call, not O(P × N).
    pub fn foreground_name(&self, root_pid: u32) -> Option<String> {
        let d = deepest_descendant_with_map(&self.children, root_pid)?;
        name_of(&self.procs, d)
    }

    /// Test-only constructor: build a Snapshot from a synthetic Vec<Proc>.
    #[cfg(test)]
    pub fn from_procs(procs: Vec<Proc>) -> Self {
        let children = build_children_map(&procs);
        Self { procs, children }
    }
}

fn build_children_map(procs: &[Proc]) -> HashMap<u32, Vec<u32>> {
    let mut children: HashMap<u32, Vec<u32>> = HashMap::new();
    for p in procs {
        children.entry(p.ppid).or_default().push(p.pid);
    }
    children
}

/// Deepest descendant of `root` using a prebuilt children map.
fn deepest_descendant_with_map(children: &HashMap<u32, Vec<u32>>, root: u32) -> Option<u32> {
    let mut best: Option<u32> = None;
    let mut best_depth = 0usize;
    // RT-23: a ppid can reference a recycled/foreign PID, so the children map can
    // contain a cycle (pid == ppid, or two procs each naming the other as parent).
    // Without a visited guard this DFS pushes forever and hangs the UI thread (the
    // poll runs on it every 750 ms). Skip any pid we've already expanded.
    let mut visited: std::collections::HashSet<u32> = std::collections::HashSet::new();
    let mut stack = vec![(root, 0usize)];
    while let Some((pid, depth)) = stack.pop() {
        if !visited.insert(pid) {
            continue;
        }
        if depth > best_depth || (depth == best_depth && depth > 0 && Some(pid) > best) {
            best = Some(pid);
            best_depth = depth;
        }
        if let Some(kids) = children.get(&pid) {
            for k in kids {
                stack.push((*k, depth + 1));
            }
        }
    }
    if best_depth == 0 {
        None
    } else {
        best
    }
}

/// Deepest descendant of `root` (closest thing to the foreground process).
/// Returns None when `root` has no children (i.e. just the bare shell).
///
/// Test-only wrapper that builds the children map then walks it. Production
/// reuses the prebuilt map via `Snapshot::foreground_name` (CA-38); this exposes
/// the same algorithm (including the RT-23 cycle guard) for direct pid-level
/// assertions in the unit tests below.
#[cfg(test)]
pub fn deepest_descendant(procs: &[Proc], root: u32) -> Option<u32> {
    let children = build_children_map(procs);
    deepest_descendant_with_map(&children, root)
}

pub fn name_of(procs: &[Proc], pid: u32) -> Option<String> {
    procs
        .iter()
        .find(|p| p.pid == pid)
        .map(|p| strip_exe(&p.name))
}

fn strip_exe(name: &str) -> String {
    name.strip_suffix(".exe")
        .or_else(|| name.strip_suffix(".EXE"))
        .unwrap_or(name)
        .to_string()
}

/// Name of the foreground process in the tree rooted at `root_pid`, if any.
///
/// Test-only convenience over the pure tree logic; production uses
/// `Snapshot::foreground_name` (CA-38).
#[cfg(test)]
pub fn foreground_name(procs: &[Proc], root_pid: u32) -> Option<String> {
    let d = deepest_descendant(procs, root_pid)?;
    name_of(procs, d)
}

#[cfg(windows)]
pub fn snapshot() -> Vec<Proc> {
    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
        TH32CS_SNAPPROCESS,
    };

    let mut out = Vec::new();
    unsafe {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snap == INVALID_HANDLE_VALUE {
            return out;
        }
        let mut entry: PROCESSENTRY32W = std::mem::zeroed();
        entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
        if Process32FirstW(snap, &mut entry) != 0 {
            loop {
                let end = entry.szExeFile.iter().position(|&c| c == 0).unwrap_or(0);
                let name = String::from_utf16_lossy(&entry.szExeFile[..end]);
                out.push(Proc {
                    pid: entry.th32ProcessID,
                    ppid: entry.th32ParentProcessID,
                    name,
                });
                if Process32NextW(snap, &mut entry) == 0 {
                    break;
                }
            }
        }
        CloseHandle(snap);
    }
    out
}

#[cfg(not(windows))]
pub fn snapshot() -> Vec<Proc> {
    Vec::new()
}

/// This process's working-set (RSS) in bytes and total CPU time consumed, in
/// 100 ns ticks (kernel + user). Two cheap syscalls — no snapshots, no handles
/// opened (`GetCurrentProcess` is a pseudo-handle). Feeds the tab bar's live
/// `mem · cpu` readout, sampled on the existing 750 ms process poll.
#[cfg(windows)]
pub fn self_usage() -> Option<(u64, u64)> {
    use windows_sys::Win32::Foundation::FILETIME;
    use windows_sys::Win32::System::ProcessStatus::{
        GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS,
    };
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, GetProcessTimes};

    unsafe {
        let me = GetCurrentProcess();
        let mut pmc: PROCESS_MEMORY_COUNTERS = std::mem::zeroed();
        pmc.cb = std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32;
        if GetProcessMemoryInfo(me, &mut pmc, pmc.cb) == 0 {
            return None;
        }
        let mut creation: FILETIME = std::mem::zeroed();
        let mut exit: FILETIME = std::mem::zeroed();
        let mut kernel: FILETIME = std::mem::zeroed();
        let mut user: FILETIME = std::mem::zeroed();
        if GetProcessTimes(me, &mut creation, &mut exit, &mut kernel, &mut user) == 0 {
            return None;
        }
        let ticks = |ft: FILETIME| ((ft.dwHighDateTime as u64) << 32) | ft.dwLowDateTime as u64;
        Some((
            pmc.WorkingSetSize as u64,
            ticks(kernel).saturating_add(ticks(user)),
        ))
    }
}

#[cfg(not(windows))]
pub fn self_usage() -> Option<(u64, u64)> {
    None
}

/// This process's commit charge (PagefileUsage — private bytes backed by
/// RAM+pagefile) and working set, in bytes. The commit number is what the
/// memory guard (RT-138) caps and logs: it is the resource whose exhaustion
/// took the machine down in the field, and it grows even when pages are
/// swapped out (unlike the working set).
#[cfg(windows)]
pub fn self_commit() -> Option<(u64, u64)> {
    use windows_sys::Win32::System::ProcessStatus::{
        GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS, PROCESS_MEMORY_COUNTERS_EX,
    };
    use windows_sys::Win32::System::Threading::GetCurrentProcess;

    unsafe {
        let mut pmc: PROCESS_MEMORY_COUNTERS_EX = std::mem::zeroed();
        pmc.cb = std::mem::size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32;
        if GetProcessMemoryInfo(
            GetCurrentProcess(),
            std::ptr::addr_of_mut!(pmc) as *mut PROCESS_MEMORY_COUNTERS,
            pmc.cb,
        ) == 0
        {
            return None;
        }
        Some((pmc.PagefileUsage as u64, pmc.WorkingSetSize as u64))
    }
}

#[cfg(not(windows))]
pub fn self_commit() -> Option<(u64, u64)> {
    None
}

/// Debug-only self-instrumentation: this process's working-set (RSS) in bytes
/// and its live OS thread count. Lets us tell a real heap/thread leak apart from
/// a CPU spin while diagnosing freezes — compare RSS and `os_threads` against the
/// live pane count over time. Compiled only in debug builds, so it never touches
/// the size-sensitive release binary.
#[cfg(all(windows, debug_assertions))]
pub fn self_stats() -> Option<(u64, usize)> {
    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Thread32First, Thread32Next, TH32CS_SNAPTHREAD, THREADENTRY32,
    };
    use windows_sys::Win32::System::ProcessStatus::{
        GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS,
    };
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, GetCurrentProcessId};

    unsafe {
        let pid = GetCurrentProcessId();

        // Working-set size (resident memory). GetCurrentProcess() is a pseudo-
        // handle that must not be closed.
        let mut pmc: PROCESS_MEMORY_COUNTERS = std::mem::zeroed();
        pmc.cb = std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32;
        let rss = if GetProcessMemoryInfo(GetCurrentProcess(), &mut pmc, pmc.cb) != 0 {
            pmc.WorkingSetSize as u64
        } else {
            0
        };

        // Count threads owned by this process via a thread snapshot.
        let mut threads = 0usize;
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0);
        if snap != INVALID_HANDLE_VALUE {
            let mut te: THREADENTRY32 = std::mem::zeroed();
            te.dwSize = std::mem::size_of::<THREADENTRY32>() as u32;
            if Thread32First(snap, &mut te) != 0 {
                loop {
                    if te.th32OwnerProcessID == pid {
                        threads += 1;
                    }
                    if Thread32Next(snap, &mut te) == 0 {
                        break;
                    }
                }
            }
            CloseHandle(snap);
        }

        Some((rss, threads))
    }
}

#[cfg(all(not(windows), debug_assertions))]
pub fn self_stats() -> Option<(u64, usize)> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(pid: u32, ppid: u32, name: &str) -> Proc {
        Proc {
            pid,
            ppid,
            name: name.into(),
        }
    }

    #[test]
    fn finds_deepest_descendant() {
        // shell(100) -> nvim(200) -> language-server(300)
        let procs = vec![
            p(100, 1, "pwsh.exe"),
            p(200, 100, "nvim.exe"),
            p(300, 200, "rust-analyzer.exe"),
        ];
        let d = deepest_descendant(&procs, 100).unwrap();
        assert_eq!(name_of(&procs, d).unwrap(), "rust-analyzer");
    }

    #[test]
    fn bare_shell_has_no_descendant() {
        let procs = vec![p(100, 1, "pwsh.exe")];
        assert!(deepest_descendant(&procs, 100).is_none());
    }

    #[test]
    fn foreground_name_strips_exe() {
        let procs = vec![p(100, 1, "pwsh.exe"), p(200, 100, "docker.exe")];
        assert_eq!(foreground_name(&procs, 100), Some("docker".to_string()));
    }

    #[test]
    fn snapshot_foreground_name_resolves_deepest_descendant() {
        // shell(10) -> cargo(20) -> rustc(30) -> cc(40)
        let procs = vec![
            p(10, 1, "pwsh.exe"),
            p(20, 10, "cargo.exe"),
            p(30, 20, "rustc.exe"),
            p(40, 30, "cc.exe"),
        ];
        let snap = Snapshot::from_procs(procs);
        // Deepest descendant of root 10 is cc (depth 3)
        assert_eq!(snap.foreground_name(10), Some("cc".to_string()));
    }

    #[test]
    fn snapshot_bare_shell_returns_none() {
        let procs = vec![p(10, 1, "pwsh.exe")];
        let snap = Snapshot::from_procs(procs);
        assert!(snap.foreground_name(10).is_none());
    }

    #[test]
    fn two_cycle_parent_map_terminates() {
        // RT-23: PID reuse can make two live procs each list the other as parent,
        // forming a cycle in the children map. The walk must terminate (not hang)
        // and return a bounded descendant rather than looping forever.
        let procs = vec![p(100, 200, "a.exe"), p(200, 100, "b.exe")];
        assert_eq!(deepest_descendant(&procs, 100), Some(200));
        assert_eq!(deepest_descendant(&procs, 200), Some(100));
    }

    #[test]
    fn self_parent_terminates_with_no_descendant() {
        // RT-23: a process whose ppid equals its own pid is a 1-cycle.
        let procs = vec![p(100, 1, "pwsh.exe"), p(500, 500, "loop.exe")];
        assert!(deepest_descendant(&procs, 500).is_none());
        assert!(deepest_descendant(&procs, 100).is_none());
    }

    #[test]
    fn snapshot_reuses_map_across_multiple_roots() {
        // Two independent shell trees in one snapshot
        let procs = vec![
            p(10, 1, "pwsh.exe"),
            p(20, 10, "vim.exe"),
            p(100, 1, "cmd.exe"),
            p(200, 100, "python.exe"),
        ];
        let snap = Snapshot::from_procs(procs);
        assert_eq!(snap.foreground_name(10), Some("vim".to_string()));
        assert_eq!(snap.foreground_name(100), Some("python".to_string()));
    }
}
