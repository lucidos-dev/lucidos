//! Scheduler configuration

/// Configuration for the scheduler
#[derive(Debug, Clone)]
pub struct SchedulerConfig {
    /// Maximum length of notification summary (characters)
    pub notification_summary_max_len: usize,

    /// Default cron expression for morning brief (default: 8am daily)
    pub morning_brief_cron: String,

    /// User's timezone (IANA timezone name, e.g., "America/New_York")
    /// Used by the LLM to convert user's local times to UTC cron expressions
    pub user_timezone: String,
}

impl Default for SchedulerConfig {
    fn default() -> Self {
        Self {
            notification_summary_max_len: 500,
            morning_brief_cron: "0 0 8 * * *".to_string(), // 8:00 AM daily (UTC)
            user_timezone: std::env::var("COGNOS_TIMEZONE").unwrap_or_default(),
        }
    }
}

impl SchedulerConfig {
    /// Create config with custom morning brief time
    pub fn with_morning_brief_cron(mut self, cron: String) -> Self {
        self.morning_brief_cron = cron;
        self
    }
}
