//! Native helper execution belongs to the engine, outside ELF loading.
pub fn unix_rights() -> Result<(), i32> {
    kinakaze_vfs::job::enter_internal_helper();
    kinakaze_vfs::unix::run_rights_keeper()
}

pub fn usernet() -> Result<(), i32> {
    kinakaze_vfs::job::enter_internal_helper();
    kinakaze_vfs::usernet_broker::run();
    Ok(())
}
