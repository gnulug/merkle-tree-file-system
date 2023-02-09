use fuser::{Filesystem, MountOption};
use std::env;

struct NullFS;

impl Filesystem for NullFS {}

pub fn main() {
    let mountpoint = env::args_os().nth(1).unwrap();
    fuser::mount2(NullFS, mountpoint, &[MountOption::AutoUnmount]).unwrap();
}
