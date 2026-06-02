// Detect the foreground process inside a pane by walking the shell's process
// tree. The tree logic is pure and tested; the OS snapshot is Windows-only.

pub struct Proc {
    pub pid: u32,
    pub ppid: u32,
    pub name: String,
}

/// Deepest descendant of `root` (closest thing to the foreground process).
/// Returns None when `root` has no children (i.e. just the bare shell).
pub fn deepest_descendant(procs: &[Proc], root: u32) -> Option<u32> {
    use std::collections::HashMap;
    let mut children: HashMap<u32, Vec<u32>> = HashMap::new();
    for p in procs {
        children.entry(p.ppid).or_default().push(p.pid);
    }
    let mut best: Option<u32> = None;
    let mut best_depth = 0usize;
    let mut stack = vec![(root, 0usize)];
    while let Some((pid, depth)) = stack.pop() {
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
}
