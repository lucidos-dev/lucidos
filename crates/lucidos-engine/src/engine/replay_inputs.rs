use crate::core::store::{
    format_child_report_filed_block, format_child_thread_completed_block, EventStore,
};
use crate::core::EventRow;
use crate::engine::types::RedeliveredInput;

pub struct RebuiltInput {
    pub row: EventRow,
    pub input: RedeliveredInput,
    pub text: String,
}

pub async fn rebuilt_inputs(pool: &sqlx::PgPool, rows: Vec<EventRow>) -> Vec<RebuiltInput> {
    let statuses = EventStore::new(pool.clone())
        .reported_change_statuses(&rows)
        .await;
    rows.into_iter()
        .filter_map(|row| {
            let input = RedeliveredInput::of_row(&row.event_type, &row.payload)?;
            let text = match (input, row.event_type.as_str()) {
                (RedeliveredInput::ChildReport, "ChildReportFiled") => {
                    format_child_report_filed_block(&row, &statuses)
                }
                (RedeliveredInput::ChildReport, _) => {
                    format_child_thread_completed_block(&row, &statuses)
                }
                _ => row
                    .payload
                    .get("text")
                    .and_then(|t| t.as_str())
                    .unwrap_or_default()
                    .to_string(),
            };
            Some(RebuiltInput { row, input, text })
        })
        .collect()
}
