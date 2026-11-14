//! The restricted settlement sequence shared by construction and boot tests.
use crate::core::snapshot::{first_boot::VerifiedRestoreBoot, BoxError};

/// End archived authority before announcing the restore. This function starts
/// no drivers or subscribers and does not release the restricted boot guard.
/// Every step is durable and retryable after a fresh process acquires ownership.
pub(crate) async fn settle_before_dispatch(
    bus: &super::event_bus::EventBus,
    restore_boot: Option<&VerifiedRestoreBoot<'_>>,
) -> Result<(), BoxError> {
    let pool = bus.pool();
    if let Some(restore_boot) = restore_boot {
        restore_boot.validate_boundary(pool).await?;
    }
    super::chat::recovery::settle_restored_threads(pool, bus).await?;
    super::event_wait::settle_restored_waits(pool, bus).await?;
    super::apply_all_driver::settle_restored_batches(pool, bus).await?;
    super::standing_apply::settle_restored_standing_applies(pool, bus).await?;
    super::landing_lane::settle_restored_lane(pool, bus).await?;
    super::main_check::settle_restored_checks(pool, bus).await?;
    if let Some(restore_boot) = restore_boot {
        restore_boot.announce_restored(bus).await?;
    }
    Ok(())
}

/// Final construction boundary. Consuming the capability releases archive
/// ownership before Session::recover_at_boot takes that same lock.
pub(crate) async fn complete_before_dispatch(
    bus: &super::event_bus::EventBus,
    restore_boot: Option<VerifiedRestoreBoot<'_>>,
) -> Result<(), BoxError> {
    if let Some(restore_boot) = restore_boot {
        restore_boot.complete(bus).await?;
    }
    Ok(())
}
