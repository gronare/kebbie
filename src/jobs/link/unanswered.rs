use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use kebbie_hub_protocol::Id;

pub const REMEMBER: Duration = Duration::from_secs(10 * 60);

// The machines whose announced addresses lost to the hub's relay lately, one file per machine holding those addresses, so the next link asks the relay at once.
pub struct Unanswered {
    dir: PathBuf,
}

fn listed(addresses: &[SocketAddr]) -> String {
    addresses.iter().map(SocketAddr::to_string).collect::<Vec<_>>().join("\n")
}

impl Unanswered {
    pub fn at(dir: &Path) -> Unanswered {
        Unanswered { dir: dir.to_path_buf() }
    }

    fn file(&self, machine: &Id) -> PathBuf {
        self.dir.join(super::super::safe(&machine.to_string()))
    }

    pub fn lately(&self, machine: &Id, addresses: &[SocketAddr]) -> bool {
        let file = self.file(machine);
        let young = std::fs::metadata(&file).and_then(|meta| meta.modified()).ok().and_then(|at| SystemTime::now().duration_since(at).ok()).is_some_and(|age| age < REMEMBER);
        young && std::fs::read_to_string(&file).is_ok_and(|text| text == listed(addresses))
    }

    pub fn remember(&self, machine: &Id, addresses: &[SocketAddr]) {
        let _ = crate::state::write(&self.file(machine), listed(addresses));
    }

    pub fn forget(&self, machine: &Id) {
        let _ = std::fs::remove_file(self.file(machine));
    }
}
