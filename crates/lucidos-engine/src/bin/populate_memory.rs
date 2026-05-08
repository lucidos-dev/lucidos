//! Populate a workspace with 2 years of simulated history for testing
//!
//! This populates:
//! - PostgreSQL event store (persistent event log) with backdated timestamps
//! - Memory index (vector search) with backdated timestamps
//! - Notifications (scheduler results) with backdated timestamps
//! - Sample artifacts on disk
//!
//! Usage: cargo run -p lucidos-engine --bin populate_memory [workspace_path]

use chrono::{Duration, Utc};
use lucidos_engine::core::EventStore;
use lucidos_engine::log;
use lucidos_engine::memory::{EmbeddingProvider, FastEmbedProvider, MemorySource, PgVectorIndex};
use lucidos_engine::scheduler::NotificationStore;
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use std::path::PathBuf;
use uuid::Uuid;

// Large variety of content for unique test data generation
const PROJECTS: &[&str] = &[
    "Website Redesign",
    "Q1 Sales Analysis",
    "Customer Onboarding",
    "Mobile App MVP",
    "API Migration",
    "Data Pipeline",
    "Security Audit",
    "Performance Optimization",
    "User Research",
    "Marketing Campaign",
    "Inventory System",
    "Payment Integration",
    "Analytics Dashboard",
    "Email Automation",
    "Fitness Tracker",
    "CRM Upgrade",
    "Infrastructure Migration",
    "CI/CD Pipeline",
    "Kubernetes Deployment",
    "GraphQL API",
    "React Native App",
    "Microservices Refactor",
    "Load Balancer Setup",
    "Database Sharding",
    "OAuth Implementation",
    "Logging System",
    "Metrics Dashboard",
    "A/B Testing Platform",
    "Feature Flags System",
    "Error Tracking",
    "Documentation Portal",
    "Developer Portal",
];

const PEOPLE: &[&str] = &[
    "Sarah",
    "Alex",
    "Jamie",
    "Michael",
    "Emma",
    "David",
    "Lisa",
    "Chris",
    "Rachel",
    "Tom",
    "Jessica",
    "Kevin",
    "Amanda",
    "Brian",
    "Nicole",
    "Steve",
    "Maria",
    "John",
    "Ashley",
    "Ryan",
    "Michelle",
    "Daniel",
    "Stephanie",
    "Matt",
];

const LOCATIONS: &[&str] = &[
    "Barcelona",
    "Tokyo",
    "London",
    "New York",
    "Paris",
    "Berlin",
    "Sydney",
    "Toronto",
    "Amsterdam",
    "Singapore",
    "San Francisco",
    "Seattle",
    "Austin",
    "Denver",
    "Chicago",
];

const RESTAURANTS: &[&str] = &[
    "that Italian place",
    "the new Thai restaurant",
    "Sushi Paradise",
    "The Burger Joint",
    "Olive Garden",
    "the Mexican spot",
    "Pho House",
    "Mediterranean Grill",
    "Steakhouse 55",
    "the French bistro",
    "the Indian buffet",
    "Korean BBQ",
    "that ramen shop",
    "the pizza place",
];

const BOOKS: &[&str] = &[
    "Designing Data-Intensive Applications",
    "Clean Code",
    "The Pragmatic Programmer",
    "System Design Interview",
    "Atomic Habits",
    "Deep Work",
    "Project Hail Mary",
    "The Three-Body Problem",
    "Sapiens",
    "Thinking Fast and Slow",
    "Zero to One",
    "The Lean Startup",
    "Staff Engineer",
    "Building Microservices",
    "Domain-Driven Design",
];

const SHOWS: &[&str] = &[
    "The Bear",
    "Severance",
    "House of the Dragon",
    "The Last of Us",
    "Succession",
    "Ted Lasso",
    "Andor",
    "The Mandalorian",
    "Wednesday",
    "White Lotus",
    "Shrinking",
    "For All Mankind",
    "Foundation",
    "Slow Horses",
    "Silo",
];

const HOBBIES: &[&str] = &[
    "running",
    "cycling",
    "swimming",
    "yoga",
    "weightlifting",
    "rock climbing",
    "hiking",
    "photography",
    "cooking",
    "gardening",
    "chess",
    "guitar",
    "painting",
    "pottery",
    "woodworking",
    "meditation",
    "tennis",
    "basketball",
    "golf",
    "skiing",
];

const GROCERIES: &[&str] = &[
    "milk",
    "eggs",
    "bread",
    "cheese",
    "butter",
    "chicken",
    "salmon",
    "rice",
    "pasta",
    "tomatoes",
    "avocados",
    "bananas",
    "apples",
    "oranges",
    "spinach",
    "broccoli",
    "carrots",
    "onions",
    "garlic",
    "olive oil",
    "coffee",
    "tea",
];

const SERVICES: &[&str] = &[
    "car service",
    "oil change",
    "dentist appointment",
    "doctor checkup",
    "eye exam",
    "haircut",
    "home cleaning",
    "HVAC maintenance",
    "plumber visit",
    "electrician",
    "vet appointment for Max",
    "tire rotation",
    "car wash",
    "dry cleaning pickup",
];

const BUGS: &[&str] = &[
    "null pointer in auth module",
    "race condition in cache",
    "memory leak in worker pool",
    "timeout in API calls",
    "incorrect timezone handling",
    "SQL injection vulnerability",
    "XSS in user input",
    "CORS misconfiguration",
    "JWT expiration bug",
    "pagination off-by-one",
    "deadlock in transaction",
    "cache invalidation issue",
    "encoding problem with unicode",
    "floating point rounding error",
    "connection pool exhaustion",
    "retry loop infinite",
];

const FEATURES: &[&str] = &[
    "dark mode",
    "export to PDF",
    "bulk import",
    "role-based access",
    "audit logging",
    "two-factor auth",
    "SSO integration",
    "webhook support",
    "API rate limiting",
    "real-time notifications",
    "search autocomplete",
    "keyboard shortcuts",
    "offline mode",
    "data export",
    "custom themes",
    "localization",
    "batch operations",
    "undo/redo",
];

const MODULES: &[&str] = &[
    "authentication",
    "database layer",
    "API handlers",
    "frontend components",
    "background jobs",
    "caching layer",
    "logging system",
    "error handling",
    "validation logic",
    "payment processing",
    "notification service",
    "search engine",
    "file uploads",
    "reporting module",
    "admin dashboard",
    "user management",
];

/// Generate a unique user message based on day and index
fn generate_user_message(day: i64, idx: i64, project: &str) -> String {
    let seed = ((day * 7 + idx * 13) as usize).wrapping_mul(31);
    let person = PEOPLE[seed % PEOPLE.len()];
    let bug = BUGS[(seed / 3) % BUGS.len()];
    let feature = FEATURES[(seed / 5) % FEATURES.len()];
    let module = MODULES[(seed / 7) % MODULES.len()];

    // Use day to create truly varied messages
    match (day + idx) % 60 {
        0 => format!(
            "Started deep work session on {} today, focusing on the {} module",
            project, module
        ),
        1 => format!(
            "Had a productive meeting with {} about {} - we agreed on next steps",
            person, project
        ),
        2 => format!(
            "Code review for {} PR #{} - left {} comments, mostly minor",
            project,
            100 + day,
            2 + (seed % 8)
        ),
        3 => format!(
            "Fixed {} in {} after {} hours of debugging",
            bug,
            project,
            1 + (seed % 4)
        ),
        4 => format!(
            "Deployed {} v{}.{}.{} to staging environment successfully",
            project,
            1 + (day / 100),
            (day / 10) % 10,
            day % 10
        ),
        5 => format!(
            "Wrote {} pages of documentation for {} API endpoints",
            2 + (seed % 6),
            project
        ),
        6 => format!(
            "Analyzed {} metrics - {} users active, {}ms avg response time",
            project,
            1000 + seed % 5000,
            50 + seed % 200
        ),
        7 => format!(
            "Created weekly report for {} - sprint velocity at {}%",
            project,
            70 + seed % 30
        ),
        8 => format!(
            "Updated {} roadmap for Q{} - added {} new milestones",
            project,
            1 + (day / 90) % 4,
            2 + seed % 5
        ),
        9 => format!(
            "Discussed {} blockers with {} - {} is the main issue",
            project, person, bug
        ),
        10 => format!(
            "Completed {} milestone for {}: {} feature is now live",
            feature, project, module
        ),
        11 => format!(
            "Added {} feature to {} - took {} story points",
            feature,
            project,
            3 + seed % 8
        ),
        12 => format!(
            "Refactored {} in {} - reduced code by {}%",
            module,
            project,
            15 + seed % 35
        ),
        13 => format!("Set up {} monitoring for {} using Datadog", module, project),
        14 => format!(
            "Ran load tests on {} - handles {} req/sec",
            project,
            500 + seed % 2000
        ),
        15 => format!(
            "Called {} about the {} project collaboration",
            person, project
        ),
        16 => format!(
            "Had {} today - everything went smoothly",
            SERVICES[(seed / 11) % SERVICES.len()]
        ),
        17 => format!(
            "Ordered new {} for the home office",
            [
                "monitor",
                "keyboard",
                "mouse",
                "desk lamp",
                "chair",
                "headphones"
            ][seed % 6]
        ),
        18 => format!(
            "Gym session - {} for {} minutes, felt great",
            HOBBIES[seed % 6],
            30 + seed % 60
        ),
        19 => format!(
            "Read chapter {} of {} - fascinating insights",
            1 + (day % 15),
            BOOKS[(seed / 9) % BOOKS.len()]
        ),
        20 => format!(
            "Grocery run - picked up {}, {}, and {}",
            GROCERIES[seed % GROCERIES.len()],
            GROCERIES[(seed + 3) % GROCERIES.len()],
            GROCERIES[(seed + 7) % GROCERIES.len()]
        ),
        21 => format!("Fixed the {} issue that {} reported yesterday", bug, person),
        22 => format!(
            "Booked flights to {} for the {} conference",
            LOCATIONS[seed % LOCATIONS.len()],
            ["tech", "developer", "industry", "startup"][seed % 4]
        ),
        23 => format!(
            "{} went well - no issues found",
            SERVICES[(seed / 13) % SERVICES.len()]
        ),
        24 => format!(
            "Started learning {} - completed lesson {}",
            ["Spanish", "French", "German", "Japanese", "Python", "Rust"][seed % 6],
            1 + day % 30
        ),
        25 => format!(
            "Finished {} - highly recommend it",
            BOOKS[(seed / 7) % BOOKS.len()]
        ),
        26 => format!(
            "Watched {} episode {} - great show",
            SHOWS[seed % SHOWS.len()],
            1 + seed % 10
        ),
        27 => format!(
            "Dinner with {} at {} - need to go back",
            person,
            RESTAURANTS[seed % RESTAURANTS.len()]
        ),
        28 => format!(
            "Renewed {} subscription for another year",
            ["Netflix", "Spotify", "gym", "NYT", "AWS"][seed % 5]
        ),
        29 => format!(
            "Pair programming with {} on {} - very productive",
            person, project
        ),
        30 => format!(
            "Sprint planning for {} - estimated {} story points total",
            project,
            20 + seed % 30
        ),
        31 => format!(
            "Retrospective meeting - {} action items identified for {}",
            2 + seed % 5,
            project
        ),
        32 => format!(
            "Database migration for {} completed - {} tables updated",
            project,
            3 + seed % 10
        ),
        33 => format!(
            "Security scan on {} - {} vulnerabilities found and fixed",
            project,
            seed % 5
        ),
        34 => format!(
            "Performance profiling {} - identified {} hot spots",
            project,
            2 + seed % 4
        ),
        35 => format!(
            "Interviewed candidate for {} team - {} experience",
            project,
            ["strong", "good", "promising", "excellent"][seed % 4]
        ),
        36 => format!("Onboarded {} to the {} project today", person, project),
        37 => format!("Tech debt session - cleaned up {} in {}", module, project),
        38 => format!("API design review for {} v{}", project, 2 + seed % 3),
        39 => format!(
            "Infrastructure cost review - {} is using ${}k/month",
            project,
            1 + seed % 10
        ),
        40 => format!(
            "Chaos engineering test on {} - recovery time {}s",
            project,
            5 + seed % 30
        ),
        41 => format!(
            "Feature flag rollout for {} - now at {}%",
            feature,
            10 + seed % 90
        ),
        42 => format!(
            "Customer feedback session for {} - {} positive, {} suggestions",
            project,
            3 + seed % 7,
            1 + seed % 4
        ),
        43 => format!(
            "Dependency update for {} - {} packages upgraded",
            project,
            5 + seed % 20
        ),
        44 => format!(
            "Wrote {} test cases for {} - coverage now at {}%",
            10 + seed % 30,
            project,
            75 + seed % 25
        ),
        45 => format!(
            "Demo of {} to stakeholders - {} attended",
            project,
            5 + seed % 15
        ),
        46 => format!(
            "Incident postmortem for {} outage - {} root cause",
            project, bug
        ),
        47 => format!(
            "Capacity planning for {} - scaling to {} instances",
            project,
            3 + seed % 10
        ),
        48 => format!(
            "Mentoring session with {} about {}",
            person,
            [
                "career growth",
                "technical skills",
                "leadership",
                "project management"
            ][seed % 4]
        ),
        // Car-specific messages for better searchability
        49 => format!(
            "Car service appointment at {} - they checked the {}",
            [
                "Honda dealership",
                "Toyota service center",
                "local mechanic",
                "auto shop"
            ][seed % 4],
            ["brakes", "transmission", "AC system", "check engine light"][seed % 4]
        ),
        50 => format!(
            "Oil change done - car now has {} miles. Tires look good for another {} miles.",
            45000 + (day * 50) % 15000,
            5000 + seed % 5000
        ),
        51 => format!(
            "Car inspection passed! Next one due in {}. Mileage: {} miles.",
            ["6 months", "1 year", "2 years"][seed % 3],
            45000 + (day * 50) % 15000
        ),
        52 => format!(
            "Got new {} tires installed - handles much better now in the rain.",
            ["Michelin", "Goodyear", "Continental", "Bridgestone"][seed % 4]
        ),
        53 => format!(
            "Car insurance renewal with {} - paying ${}0/month now. Saved ${}0 by switching.",
            ["State Farm", "Progressive", "Geico", "Allstate"][seed % 4],
            8 + seed % 12,
            2 + seed % 5
        ),
        54 => "Washed and detailed the car - vacuumed interior and applied wax.".to_string(),
        55 => format!(
            "Car registration renewed online for ${}. New sticker should arrive in {} days.",
            80 + seed % 50,
            5 + seed % 10
        ),
        56 => format!(
            "Dropped car off for brake pad replacement - picking it up at {}pm.",
            2 + seed % 4
        ),
        57 => format!(
            "Check engine light came on - diagnostic showed {}. Repair cost: ${}.",
            [
                "oxygen sensor",
                "catalytic converter",
                "loose gas cap",
                "spark plugs"
            ][seed % 4],
            150 + seed % 500
        ),
        58 => format!(
            "Filled up gas tank - ${}. Car gets about {} mpg in the city.",
            35 + seed % 40,
            25 + seed % 15
        ),
        59 => format!(
            "Car battery replaced after it died this morning. New battery has {} year warranty.",
            2 + seed % 4
        ),
        _ => format!(
            "Worked on {} today - {} with {}",
            project,
            [
                "good progress",
                "some challenges",
                "breakthrough",
                "steady work"
            ][seed % 4],
            person
        ),
    }
}

fn generate_assistant_response(day: i64, idx: i64, project: &str) -> String {
    let seed = ((day * 11 + idx * 17) as usize).wrapping_mul(37);
    let person = PEOPLE[(seed + 5) % PEOPLE.len()];

    match (day + idx) % 40 {
        0 => format!("Noted your {} session. I've added this to your project timeline. Any blockers to track?", project),
        1 => format!("Got it! I've logged the meeting with {}. Should I send a follow-up reminder?", person),
        2 => format!("Code review logged. Your {} PR stats: {} reviews this week.", project, 2 + seed % 5),
        3 => format!("Bug fix recorded for {}. I'll add it to the release notes. Great debugging work!", project),
        4 => format!("Deployment noted! {} is now on staging. Want me to schedule the prod deploy?", project),
        5 => format!("Documentation update tracked. Your {} docs are {} pages now.", project, 10 + seed % 40),
        6 => format!("Metrics logged for {}. Compared to last week: {}% improvement.", project, 5 + seed % 20),
        7 => format!("Weekly report filed. Sprint velocity is trending {}.", ["up", "steady", "slightly down"][seed % 3]),
        8 => format!("Roadmap updated. Q{} milestones now visible to the team.", 1 + (day / 90) % 4),
        9 => format!("Blocker documented. I'll ping {} if it's not resolved by tomorrow.", person),
        10 => format!("Milestone completed! {} is {} done overall.", project, 40 + seed % 60),
        11 => format!("Feature logged. Your {} velocity is impressive this sprint.", project),
        12 => format!("Refactoring noted. Code quality metrics improved for {}.", project),
        13 => "Monitoring setup recorded. I'll alert you if anything looks off.".to_string(),
        14 => format!("Load test results saved. {} is performing {} than expected.", project, ["better", "as expected", "slightly below"][seed % 3]),
        15 => "Personal call noted. I've blocked your calendar for the call.".to_string(),
        16 => format!("Appointment scheduled. I'll remind you {} before.", ["1 hour", "the day", "2 hours"][seed % 3]),
        17 => "Purchase logged. Your home office setup is coming together nicely!".to_string(),
        18 => format!("Workout tracked! You've exercised {} times this week.", 2 + seed % 5),
        19 => format!("Reading progress updated. You're {} through the book now.", ["halfway", "almost done", "a quarter"][seed % 3]),
        20 => "Shopping list updated. Need anything else while you're out?".to_string(),
        21 => format!("Bug fix from {} logged. Your team has fixed {} issues this sprint.", person, 5 + seed % 10),
        22 => "Travel booked! I'll prepare a packing list closer to the date.".to_string(),
        23 => format!("Appointment logged as complete. Next one scheduled for {}.", ["next month", "3 months", "6 months"][seed % 3]),
        24 => format!("Learning progress tracked! You're on a {} day streak.", 5 + day % 30),
        25 => format!("Book finished! Added to your reading list. {} books this year.", 5 + seed % 20),
        26 => format!("Episode logged. You're caught up on {}!", SHOWS[seed % SHOWS.len()]),
        27 => format!("Dinner noted. {} is now in your favorites.", RESTAURANTS[seed % RESTAURANTS.len()]),
        28 => format!("Subscription renewal tracked. Your monthly subscriptions total ${}.", 30 + seed % 100),
        // Car-related responses
        29 => format!("Car service logged. I'll remind you to pick it up. Your car is at {} miles now.", 45000 + (seed * 50) % 15000),
        30 => format!("Oil change recorded. Based on your driving, next one in about {} months.", 3 + seed % 4),
        31 => format!("Car inspection passed - great! Next due in {}. I've added a reminder.", ["6 months", "1 year", "2 years"][seed % 3]),
        32 => "New tires logged. Your car maintenance is up to date!".to_string(),
        33 => format!("Car insurance updated. You're saving ${}/month compared to before.", 20 + seed % 50),
        34 => "Car detailed - nice! I've noted this in your vehicle history.".to_string(),
        35 => "Registration renewed. Sticker reminder cleared from your tasks.".to_string(),
        36 => format!("Brake service logged. Safety first! Car should be ready by {}pm.", 2 + seed % 4),
        37 => "Check engine issue tracked. I'll remind you about the repair follow-up.".to_string(),
        38 => format!("Gas fill-up logged. You've spent ${} on fuel this month.", 100 + seed % 150),
        39 => format!("Battery replacement noted. New warranty expires in {} years.", 2 + seed % 4),
        _ => format!("Got it! {} progress noted. Anything else to track for today?", project),
    }
}

fn generate_morning_brief(day: i64) -> String {
    let seed = (day as usize).wrapping_mul(41);
    let project1 = PROJECTS[seed % PROJECTS.len()];
    let project2 = PROJECTS[(seed + 7) % PROJECTS.len()];
    let person = PEOPLE[seed % PEOPLE.len()];
    let task_count = 2 + seed % 5;
    let meeting_time = 9 + seed % 4;

    match day % 25 {
        0 => format!("Good morning! Today you have {} tasks due: focus on {} and {}. You made great progress on {} yesterday.", task_count, project1, project2, project1),
        1 => format!("Rise and shine! Meeting with {} at {}am about {}. Don't forget to review the {} PR.", person, meeting_time, project1, project2),
        2 => format!("Morning! Busy day ahead: {} meetings scheduled. {} demo is tomorrow. Check your {} backlog.", 2 + seed % 3, project1, project2),
        3 => format!("Good morning! {} sprint ends Friday. You completed {} tasks yesterday. {} standup at {}am.", project1, 2 + seed % 4, project2, meeting_time),
        4 => format!("Hello! You finished the {} report yesterday. Today: {} testing and lunch with {} at noon.", project1, project2, person),
        5 => format!("Good morning! {} is at {}% progress. Today's focus: {} and code reviews.", project1, 60 + seed % 40, project2),
        6 => format!("Rise and shine! Reminder: {} deadline in {} days. {} needs your review today.", project1, 3 + seed % 7, project2),
        7 => format!("Morning! {} with {} yesterday went well. Today: {} fixes and {} planning.", ["meeting", "call", "sync"][seed % 3], person, project1, project2),
        8 => format!("Good morning! {} users reported {} issue in {}. {} deployment scheduled for {}pm.", 5 + seed % 10, BUGS[(seed / 3) % BUGS.len()], project1, project2, 2 + seed % 4),
        9 => format!("Hello! Yesterday: {} merged, {} reviewed. Today: {} and {} with {}.", project1, project2, ["deep work", "meetings", "reviews"][seed % 3], ["planning", "testing", "docs"][seed % 3], person),
        10 => format!("Good morning! {} milestone completed! Today's priorities: {} and prepare for {} demo.", project1, project2, project1),
        11 => format!("Rise and shine! {} retrospective at {}am. {} needs {} by end of day.", project1, meeting_time, project2, ["testing", "review", "approval"][seed % 3]),
        12 => format!("Morning! Q{} planning starts today. {} and {} are top priorities.", 1 + (day / 90) % 4, project1, project2),
        13 => format!("Good morning! {} is {} ahead of schedule. {} has {} items in backlog.", project1, ["1 week", "2 days", "3 days"][seed % 3], project2, 5 + seed % 15),
        14 => format!("Hello! Interview candidate at {}am for {} team. {} standup after.", meeting_time + 1, project1, project2),
        15 => format!("Good morning! {} launch in {} days. Today: final {} testing and {} check.", project1, 5 + seed % 10, project2, ["security", "performance", "integration"][seed % 3]),
        16 => format!("Rise and shine! {} code freeze tomorrow. {} PR needs merge today. {} at {}.", project1, project2, ["Call with stakeholders", "Team sync", "Customer demo"][seed % 3], meeting_time),
        17 => format!("Morning! {} downtime scheduled for {}pm maintenance. {} work continues.", project1, 2 + seed % 4, project2),
        18 => format!("Good morning! {} metrics: {}% uptime, {}ms latency. {} focus today.", project1, 99 + (seed % 2), 50 + seed % 100, project2),
        // Car-related morning briefs
        19 => format!("Good morning! Reminder: car service appointment at {}am today. After that, {} standup.", 9 + seed % 3, project1),
        20 => format!("Rise and shine! Your car insurance payment of ${} is due this week. {} work continues.", 80 + seed % 100, project1),
        21 => format!("Morning! Car registration expires in {} days - renew online. {} meeting at {}pm.", 5 + seed % 10, project1, 2 + seed % 3),
        22 => format!("Good morning! Oil change reminder - car is at {} miles. {} sprint starts today.", 45000 + (seed * 50) % 15000, project1),
        23 => format!("Hello! Car inspection passed - good for another year. Today: {} and {} with {}.", project1, project2, person),
        _ => format!("Hello! {} and {} are your main focus. {} mentioned needing help with {}.", project1, project2, person, ["testing", "reviews", "planning"][seed % 3]),
    }
}

/// Event struct with backdated timestamps
#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
struct BackdatedEvent {
    id: Uuid,
    event_type: String,
    payload: serde_json::Value,
    created: chrono::DateTime<Utc>,
}

impl BackdatedEvent {
    fn new(event_type: &str, payload: serde_json::Value, created: chrono::DateTime<Utc>) -> Self {
        Self {
            id: Uuid::new_v4(),
            event_type: event_type.to_string(),
            payload,
            created,
        }
    }

    fn user_message(request_id: Uuid, content: &str, created: chrono::DateTime<Utc>) -> Self {
        Self::new(
            "MessageReceived",
            serde_json::json!({
                "request_id": request_id.to_string(),
                "thread_id": request_id.to_string(),
                "content": content
            }),
            created,
        )
    }

    fn assistant_response(request_id: Uuid, content: &str, created: chrono::DateTime<Utc>) -> Self {
        Self::new(
            "ResponseGenerated",
            serde_json::json!({
                "request_id": request_id.to_string(),
                "thread_id": request_id.to_string(),
                "content": content
            }),
            created,
        )
    }

    fn trigger_completed(
        request_id: Uuid,
        trigger_id: Uuid,
        trigger_name: &str,
        result_summary: &str,
        created: chrono::DateTime<Utc>,
    ) -> Self {
        Self::new(
            "TriggerCompleted",
            serde_json::json!({
                "request_id": request_id.to_string(),
                "thread_id": request_id.to_string(),
                "trigger_id": trigger_id.to_string(),
                "trigger_name": trigger_name,
                "result_summary": result_summary,
                "channel": "trigger"
            }),
            created,
        )
    }
}

/// Insert backdated event directly
async fn append_backdated_event(
    pool: &sqlx::PgPool,
    event: &BackdatedEvent,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        r#"
        INSERT INTO events (id, event_type, payload, created)
        VALUES ($1, $2, $3, $4)
        "#,
    )
    .bind(event.id)
    .bind(&event.event_type)
    .bind(&event.payload)
    .bind(event.created)
    .execute(pool)
    .await?;

    Ok(())
}

/// Generate realistic artifacts based on project type
fn generate_artifact_with_content(day: i64, idx: i64, project: &str) -> (String, String, String) {
    let seed = ((day * 7 + idx * 13) as usize).wrapping_mul(31);
    let base_date = Utc::now() - Duration::days(730);
    let date = (base_date + Duration::days(day)).format("%Y-%m-%d");
    let week = ((day / 7) % 52) + 1;
    let project_slug = project.to_lowercase().replace([' ', '/'], "-");
    let person = PEOPLE[seed % PEOPLE.len()];
    let person2 = PEOPLE[(seed + 5) % PEOPLE.len()];

    // Generate project-appropriate artifacts
    match project {
        "Fitness Tracker" => generate_fitness_artifact(day, idx, seed, &date.to_string()),
        "Q1 Sales Analysis" | "Marketing Campaign" | "User Research" => {
            generate_analysis_artifact(day, idx, seed, project, &date.to_string(), person, person2)
        }
        "API Migration" | "GraphQL API" | "Microservices Refactor" => generate_technical_artifact(
            day,
            idx,
            seed,
            project,
            &project_slug,
            &date.to_string(),
            week,
        ),
        "Security Audit" => generate_security_artifact(day, idx, seed, &date.to_string()),
        _ => generate_general_artifact(
            day,
            idx,
            seed,
            project,
            &project_slug,
            &date.to_string(),
            week,
            person,
        ),
    }
}

fn generate_fitness_artifact(
    day: i64,
    idx: i64,
    seed: usize,
    date: &str,
) -> (String, String, String) {
    match (day + idx) % 4 {
        0 => {
            let distance = 3.0 + (seed % 80) as f64 / 10.0;
            let pace = 5.0 + (seed % 20) as f64 / 10.0;
            let calories = 200 + seed % 400;
            let hr_avg = 140 + seed % 30;
            (
                format!("fitness/workouts/{}-run.md", date),
                format!("Running workout on {} - {:.1}km", date, distance),
                format!(
                    r#"# Running Workout - {}

## Summary
- **Distance**: {:.1} km
- **Duration**: {} minutes
- **Pace**: {:.1} min/km
- **Calories**: {} kcal

## Heart Rate
- Average: {} bpm
- Max: {} bpm
- Zone 2 time: {} minutes

## Route
Started from home, ran through Central Park, loop around the lake, back via the west side trail. Weather was {} and {}°C.

## Notes
Felt {} today. {} were a bit tight during the first kilometer but loosened up after warming up. Need to focus on keeping cadence above 170 for the next session.

## Splits
| Km | Pace | HR |
|----|------|-----|
| 1 | {:.1} | {} |
| 2 | {:.1} | {} |
| 3 | {:.1} | {} |
| 4 | {:.1} | {} |
| 5 | {:.1} | {} |

## Recovery
- Stretched for 10 minutes post-run
- Foam rolled quads and IT band
- Protein shake within 30 minutes
"#,
                    date,
                    distance,
                    (distance * pace) as i32,
                    pace,
                    calories,
                    hr_avg,
                    hr_avg + 20,
                    (distance * pace * 0.6) as i32,
                    ["sunny", "cloudy", "overcast", "partly cloudy"][seed % 4],
                    12 + seed % 15,
                    ["strong", "good", "tired", "energetic"][seed % 4],
                    ["Hamstrings", "Calves", "Quads", "Hip flexors"][seed % 4],
                    pace - 0.2,
                    hr_avg - 5,
                    pace + 0.1,
                    hr_avg,
                    pace,
                    hr_avg + 5,
                    pace - 0.1,
                    hr_avg + 10,
                    pace + 0.2,
                    hr_avg + 15,
                ),
            )
        }
        1 => {
            let weight = 70.0 + (seed % 100) as f64 / 10.0;
            (
                format!("fitness/measurements/{}.md", date),
                format!("Body measurements recorded on {}", date),
                format!(
                    r#"# Body Measurements - {}

## Weight & Composition
- **Weight**: {:.1} kg
- **Body Fat**: {:.1}%
- **Muscle Mass**: {:.1} kg
- **Water**: {:.1}%

## Measurements (cm)
- Chest: {}
- Waist: {}
- Hips: {}
- Thigh (R): {}
- Arm (R): {}

## Progress vs Last Week
- Weight: {} kg
- Body fat: {} percentage points
- Waist: {} cm

## Notes
{}. Diet has been {} this week. Sleep averaging {} hours per night.

## Goals
- Target weight: 72 kg
- Target body fat: 15%
- Timeline: 3 months
"#,
                    date,
                    weight,
                    18.0 + (seed % 80) as f64 / 10.0,
                    weight * 0.45,
                    55.0 + (seed % 100) as f64 / 10.0,
                    95 + seed % 15,
                    80 + seed % 15,
                    95 + seed % 10,
                    55 + seed % 10,
                    35 + seed % 5,
                    ["-0.3", "+0.1", "-0.5", "0.0"][seed % 4],
                    ["-0.2", "+0.1", "-0.3", "0.0"][seed % 4],
                    ["-0.5", "0.0", "-1.0", "+0.5"][seed % 4],
                    [
                        "Feeling good about progress",
                        "Slight plateau this week",
                        "Great results lately",
                        "Need to be more consistent"
                    ][seed % 4],
                    ["on point", "mostly good", "inconsistent", "excellent"][seed % 4],
                    6 + seed % 3,
                ),
            )
        }
        2 => (
            format!("fitness/workouts/{}-strength.md", date),
            format!("Strength training session on {}", date),
            format!(
                r#"# Strength Training - {}

## Workout Type
{} - {} training day

## Warm-up
- 5 min rowing machine
- Dynamic stretches
- Activation exercises

## Main Workout

### Compound Movements
| Exercise | Sets x Reps | Weight | Notes |
|----------|-------------|--------|-------|
| {} | 4 x {} | {} kg | {} |
| {} | 4 x {} | {} kg | {} |
| {} | 3 x {} | {} kg | {} |

### Accessory Work
| Exercise | Sets x Reps | Weight |
|----------|-------------|--------|
| {} | 3 x 12 | {} kg |
| {} | 3 x 15 | {} kg |
| {} | 3 x 12 | {} kg |

## Cardio Finisher
- {} minutes on {}
- Heart rate: {} bpm average

## Recovery
- Cool down stretches: 10 minutes
- Foam rolling: focused on {}
- Post-workout nutrition: protein shake + banana

## Notes
{}. Energy levels were {}. Need to work on {} form next session.

## Progressive Overload
- {} increased by {} kg from last week
- Targeting {} next session
"#,
                date,
                ["Push", "Pull", "Legs", "Upper Body"][seed % 4],
                ["hypertrophy", "strength", "power", "endurance"][seed % 4],
                ["Bench Press", "Squat", "Deadlift", "Overhead Press"][seed % 4],
                6 + seed % 6,
                60 + seed % 40,
                [
                    "Good form",
                    "Last rep was tough",
                    "Felt strong",
                    "Need spotter next time"
                ][seed % 4],
                ["Rows", "Romanian Deadlift", "Leg Press", "Pull-ups"][seed % 4],
                8 + seed % 4,
                50 + seed % 30,
                ["Solid", "PR attempt", "Deload week", "Building back up"][seed % 4],
                ["Dips", "Lunges", "Face Pulls", "Calf Raises"][seed % 4],
                10 + seed % 6,
                40 + seed % 20,
                [
                    "Controlled tempo",
                    "Explosive",
                    "Slow negatives",
                    "Paused reps"
                ][seed % 4],
                [
                    "Lateral Raises",
                    "Tricep Extensions",
                    "Bicep Curls",
                    "Leg Curls"
                ][seed % 4],
                10 + seed % 15,
                [
                    "Cable Flies",
                    "Hammer Curls",
                    "Skull Crushers",
                    "Leg Extensions"
                ][seed % 4],
                15 + seed % 10,
                ["Plank", "Ab Wheel", "Hanging Leg Raises", "Russian Twists"][seed % 4],
                0,
                10 + seed % 15,
                ["treadmill", "bike", "rowing", "stair master"][seed % 4],
                130 + seed % 20,
                ["lats", "quads", "chest", "shoulders"][seed % 4],
                [
                    "Great session overall",
                    "Felt a bit fatigued",
                    "New PR today",
                    "Consistent progress"
                ][seed % 4],
                ["high", "moderate", "low", "excellent"][seed % 4],
                ["squat", "deadlift", "bench", "overhead press"][seed % 4],
                ["Bench Press", "Squat", "Deadlift", "Rows"][seed % 4],
                2.5,
                (62 + seed % 40) as f64 + 2.5,
            ),
        ),
        _ => (
            format!("fitness/nutrition/{}.md", date),
            format!("Nutrition log for {}", date),
            format!(
                r#"# Nutrition Log - {}

## Daily Totals
- **Calories**: {} kcal (target: 2200)
- **Protein**: {}g (target: 150g)
- **Carbs**: {}g
- **Fat**: {}g
- **Fiber**: {}g
- **Water**: {} liters

## Meals

### Breakfast (7:30 AM)
- {} with {}
- {}
- Black coffee
- Calories: {} | Protein: {}g

### Lunch (12:30 PM)
- {}
- {} on the side
- {}
- Calories: {} | Protein: {}g

### Snack (3:30 PM)
- {}
- {}
- Calories: {} | Protein: {}g

### Dinner (7:00 PM)
- {} with {}
- {}
- {}
- Calories: {} | Protein: {}g

### Evening Snack (9:00 PM)
- {}
- Calories: {} | Protein: {}g

## Supplements
- Multivitamin: ✓
- Omega-3: ✓
- Vitamin D: ✓
- Creatine: {}g

## Notes
{}. Hunger levels were {}. {} today.

## Meal Prep for Tomorrow
- Prepped {} for lunch
- Defrosted {} for dinner
"#,
                date,
                1800 + seed % 600,
                120 + seed % 50,
                180 + seed % 100,
                60 + seed % 40,
                20 + seed % 15,
                2 + seed % 2,
                ["Oatmeal", "Greek yogurt", "Eggs", "Smoothie bowl"][seed % 4],
                ["berries", "banana", "honey", "protein powder"][seed % 4],
                [
                    "Toast with avocado",
                    "Overnight oats",
                    "Protein shake",
                    "Fruit salad"
                ][seed % 4],
                350 + seed % 150,
                25 + seed % 15,
                [
                    "Grilled chicken salad",
                    "Turkey sandwich",
                    "Salmon bowl",
                    "Chicken stir-fry"
                ][seed % 4],
                ["Brown rice", "Sweet potato", "Quinoa", "Mixed vegetables"][seed % 4],
                ["Sparkling water", "Green tea", "Kombucha", "Fresh juice"][seed % 4],
                500 + seed % 200,
                40 + seed % 20,
                [
                    "Apple with almond butter",
                    "Protein bar",
                    "Cottage cheese",
                    "Trail mix"
                ][seed % 4],
                ["Carrot sticks", "Rice cakes", "Beef jerky", "Hummus"][seed % 4],
                200 + seed % 100,
                15 + seed % 10,
                [
                    "Grilled steak",
                    "Baked salmon",
                    "Chicken breast",
                    "Lean beef"
                ][seed % 4],
                [
                    "roasted vegetables",
                    "mashed potatoes",
                    "asparagus",
                    "broccoli"
                ][seed % 4],
                [
                    "Side salad",
                    "Steamed greens",
                    "Sautéed mushrooms",
                    "Roasted cauliflower"
                ][seed % 4],
                [
                    "Glass of red wine",
                    "Sparkling water",
                    "Herbal tea",
                    "Fresh lemonade"
                ][seed % 4],
                600 + seed % 200,
                45 + seed % 20,
                [
                    "Casein protein shake",
                    "Greek yogurt",
                    "Cottage cheese",
                    "Handful of almonds"
                ][seed % 4],
                150 + seed % 100,
                20 + seed % 15,
                5,
                [
                    "Hit protein target",
                    "Slightly under on calories",
                    "Good balance today",
                    "Need more vegetables"
                ][seed % 4],
                ["manageable", "high", "low", "moderate"][seed % 4],
                [
                    "No cravings",
                    "Craved sweets",
                    "Felt satisfied",
                    "Slight hunger before dinner"
                ][seed % 4],
                [
                    "chicken and rice",
                    "salmon salad",
                    "beef stew",
                    "turkey wraps"
                ][seed % 4],
                ["chicken", "fish", "beef", "pork"][seed % 4],
            ),
        ),
    }
}

#[allow(clippy::format_in_format_args)]
fn generate_analysis_artifact(
    day: i64,
    idx: i64,
    seed: usize,
    project: &str,
    date: &str,
    person: &str,
    person2: &str,
) -> (String, String, String) {
    let project_slug = project.to_lowercase().replace([' ', '/'], "-");
    match (day + idx) % 3 {
        0 => {
            (
                format!("analysis/{}/quarterly-report-q{}-{}.md", project_slug, 1 + (day / 90) % 4, date.split('-').next().unwrap_or("2024")),
                format!("Quarterly analysis report for {}", project),
                format!(
r#"# {} - Q{} {} Analysis Report

## Executive Summary
This quarter showed {} performance across key metrics. Revenue grew by {}% compared to Q{}, while customer acquisition costs decreased by {}%. The team successfully launched {} new initiatives and closed {} enterprise deals.

## Key Performance Indicators

### Revenue Metrics
| Metric | Q{} | Q{} | Change |
|--------|-----|-----|--------|
| Total Revenue | ${}M | ${}M | +{}% |
| MRR | ${}k | ${}k | +{}% |
| ARR | ${}M | ${}M | +{}% |
| ARPU | ${} | ${} | +{}% |

### Customer Metrics
| Metric | Value | Target | Status |
|--------|-------|--------|--------|
| New Customers | {} | {} | {} |
| Churn Rate | {}% | <5% | {} |
| NPS Score | {} | >50 | {} |
| CAC | ${} | <${} | {} |
| LTV | ${} | >${} | {} |

## Market Analysis

### Competitive Landscape
{}

### Market Trends
1. **{}**: {} impact on our positioning
2. **{}**: Opportunity for {} expansion
3. **{}**: Requires {} adjustment to strategy

## Channel Performance

### Marketing Channels
| Channel | Spend | Leads | CAC | ROI |
|---------|-------|-------|-----|-----|
| Paid Search | ${}k | {} | ${} | {}x |
| Social Media | ${}k | {} | ${} | {}x |
| Content | ${}k | {} | ${} | {}x |
| Events | ${}k | {} | ${} | {}x |

## Team Performance
- {} led the {} initiative, resulting in {}
- {} improved {} process, saving {} hours/week
- Cross-functional collaboration between {} and {} teams

## Challenges & Risks
1. {} - Mitigation: {}
2. {} - Mitigation: {}
3. {} - Mitigation: {}

## Q{} Priorities
1. Launch {} by end of {}
2. Achieve {}% growth in {}
3. Reduce {} by {}%
4. Expand into {} market

## Resource Requests
- {} additional headcount for {} team
- ${} budget increase for {}
- New {} tooling investment

## Appendix
- Detailed financial breakdown: See attachment A
- Customer feedback summary: See attachment B
- Competitive analysis: See attachment C

---
*Report prepared by {} and {}*
*Last updated: {}*
"#,
                    project, 1 + (day / 90) % 4, date.split('-').next().unwrap_or("2024"),
                    ["strong", "solid", "exceptional", "steady"][seed % 4],
                    12 + seed % 20, (day / 90) % 4, 8 + seed % 15,
                    2 + seed % 4, 5 + seed % 10,
                    (day / 90) % 4, 1 + (day / 90) % 4,
                    2 + seed % 5, 3 + seed % 5, 10 + seed % 30,
                    200 + seed % 300, 250 + seed % 300, 15 + seed % 25,
                    3 + seed % 4, 4 + seed % 4, 12 + seed % 20,
                    45 + seed % 30, 50 + seed % 30, 8 + seed % 15,
                    150 + seed % 200, 200, ["✓ Met", "⚠ Close", "✓ Exceeded"][seed % 3],
                    3 + seed % 4, ["✓ Good", "⚠ Watch", "✓ Excellent"][seed % 3],
                    45 + seed % 30, ["✓ Met", "✓ Exceeded", "⚠ Close"][seed % 3],
                    150 + seed % 100, 200, ["✓ Met", "⚠ Over", "✓ Under"][seed % 3],
                    1500 + seed % 1000, 1200, ["✓ Good", "✓ Excellent", "⚠ Monitor"][seed % 3],
                    ["Competitor X launched similar feature; we maintain differentiation through superior UX",
                     "Market consolidation continues; positioned well for acquisition opportunities",
                     "New entrant Y gaining traction; monitoring closely"][seed % 3],
                    ["AI/ML adoption", "Remote work tools", "Sustainability focus"][seed % 3],
                    ["positive", "neutral", "significant"][seed % 3],
                    ["Enterprise segment", "SMB market", "International"][seed % 3],
                    ["product", "sales", "support"][seed % 3],
                    ["Regulatory changes", "Platform shifts", "Economic conditions"][seed % 3],
                    ["minor", "moderate", "strategic"][seed % 3],
                    50 + seed % 100, 400 + seed % 300, 80 + seed % 50, 2 + seed % 3,
                    30 + seed % 50, 300 + seed % 200, 100 + seed % 80, 3 + seed % 4,
                    20 + seed % 30, 500 + seed % 400, 40 + seed % 30, 5 + seed % 5,
                    40 + seed % 60, 100 + seed % 100, 200 + seed % 150, 2 + seed % 2,
                    person, ["product launch", "sales enablement", "customer success"][seed % 3],
                    ["15% conversion increase", "3 enterprise wins", "$500k pipeline"][seed % 3],
                    person2, ["onboarding", "support ticket", "sales"][seed % 3], 5 + seed % 10,
                    ["Sales", "Marketing", "Product"][seed % 3], ["Engineering", "Support", "Design"][seed % 3],
                    ["Supply chain delays", "Talent acquisition", "Market volatility"][seed % 3],
                    ["diversified suppliers", "increased recruiting budget", "hedging strategy"][seed % 3],
                    ["Competitive pressure", "Technical debt", "Regulatory compliance"][seed % 3],
                    ["accelerated roadmap", "dedicated sprint", "legal review"][seed % 3],
                    ["Customer churn", "Integration complexity", "Scaling infrastructure"][seed % 3],
                    ["retention program", "API improvements", "cloud migration"][seed % 3],
                    2 + (day / 90) % 4,
                    ["new pricing tier", "mobile app v2", "enterprise features"][seed % 3],
                    ["Q1", "Q2", "March"][seed % 3],
                    15 + seed % 20, ["customer base", "revenue", "engagement"][seed % 3],
                    ["churn", "CAC", "support tickets"][seed % 3], 10 + seed % 15,
                    ["APAC", "EMEA", "LATAM"][seed % 3],
                    2 + seed % 4, ["engineering", "sales", "marketing"][seed % 3],
                    50 + seed % 100, ["marketing", "R&D", "infrastructure"][seed % 3],
                    ["analytics", "CRM", "automation"][seed % 3],
                    person, person2, date,
                )
            )
        }
        1 => {
            (
                format!("analysis/{}/customer-insights-{}.md", project_slug, date),
                format!("Customer insights and feedback analysis for {}", project),
                format!(
r#"# Customer Insights Report - {}

## Overview
Analysis of {} customer feedback responses collected between {} and {}.

## Satisfaction Scores

### Overall NPS: {}
| Category | Score | Trend |
|----------|-------|-------|
| Product | {} | {} |
| Support | {} | {} |
| Value | {} | {} |
| Ease of Use | {} | {} |

## Key Themes from Feedback

### What Customers Love
1. **{}** - Mentioned by {}% of respondents
   > "{}"

2. **{}** - Mentioned by {}% of respondents
   > "{}"

3. **{}** - Mentioned by {}% of respondents
   > "{}"

### Areas for Improvement
1. **{}** - {}% of feedback
   - Current state: {}
   - Recommended action: {}
   - Priority: {}

2. **{}** - {}% of feedback
   - Current state: {}
   - Recommended action: {}
   - Priority: {}

## Customer Segments Analysis

### Enterprise (${}/year+)
- Satisfaction: {}%
- Key needs: {}, {}
- Churn risk: {}

### Mid-Market (${}k-${}k/year)
- Satisfaction: {}%
- Key needs: {}, {}
- Growth opportunity: {}

### SMB (<${}k/year)
- Satisfaction: {}%
- Key needs: {}, {}
- Self-serve adoption: {}%

## Feature Requests (Top 10)
| Rank | Feature | Votes | Segment | Status |
|------|---------|-------|---------|--------|
| 1 | {} | {} | Enterprise | {} |
| 2 | {} | {} | All | {} |
| 3 | {} | {} | Mid-Market | {} |
| 4 | {} | {} | SMB | {} |
| 5 | {} | {} | Enterprise | {} |

## Competitive Mentions
- {} mentioned {}x ({}% of churned customers cited as reason)
- {} mentioned {}x (mainly for {} feature)
- {} mentioned {}x (price comparison)

## Recommendations
1. **Immediate**: {} to address {} concern
2. **Short-term**: {} to improve {} experience
3. **Long-term**: {} to capture {} segment

## Next Steps
- [ ] Share findings with product team by {}
- [ ] Schedule {} customer interviews
- [ ] Create {} improvement roadmap
- [ ] Present to leadership on {}

---
*Analysis by {} | Data period: {} to {}*
"#,
                    date,
                    200 + seed % 500,
                    format!("{}-{:02}-01", date.split('-').next().unwrap_or("2024"), 1 + (seed % 12)),
                    date,
                    40 + seed % 40,
                    7 + seed % 3, ["↑", "→", "↓"][seed % 3],
                    8 + seed % 2, ["↑", "→", "↑"][seed % 3],
                    6 + seed % 3, ["→", "↑", "↓"][seed % 3],
                    7 + seed % 3, ["↑", "↑", "→"][seed % 3],
                    ["Intuitive interface", "Fast performance", "Excellent support"][seed % 3],
                    60 + seed % 25,
                    ["The dashboard is so easy to navigate", "Response times are incredible", "Support team always goes above and beyond"][seed % 3],
                    ["Powerful integrations", "Reliable uptime", "Comprehensive reporting"][seed % 3],
                    45 + seed % 30,
                    ["Connects perfectly with our existing tools", "Haven't had downtime in months", "Reports give us exactly what we need"][seed % 3],
                    ["Great value for money", "Regular updates", "Strong security"][seed % 3],
                    30 + seed % 25,
                    ["ROI was evident within first month", "Love seeing new features regularly", "Feel confident about data protection"][seed % 3],
                    ["Mobile experience", "Advanced analytics", "Onboarding flow"][seed % 3],
                    25 + seed % 20,
                    ["Basic functionality only", "Limited customization", "Self-serve but complex"][seed % 3],
                    ["Mobile app redesign", "Add custom dashboards", "Interactive tutorials"][seed % 3],
                    ["High", "Medium", "High"][seed % 3],
                    ["API documentation", "Bulk operations", "SSO options"][seed % 3],
                    15 + seed % 15,
                    ["Adequate but dated", "Manual workarounds needed", "Enterprise-only currently"][seed % 3],
                    ["Developer portal revamp", "Batch processing feature", "Expand SSO providers"][seed % 3],
                    ["Medium", "High", "Medium"][seed % 3],
                    50 + seed % 50, 85 + seed % 10,
                    ["Custom workflows", "Dedicated support"][seed % 2],
                    ["Advanced security", "SLA guarantees"][seed % 2],
                    ["Low", "Medium", "Low"][seed % 3],
                    10 + seed % 20, 50 + seed % 50, 75 + seed % 15,
                    ["Automation features", "Team collaboration"][seed % 2],
                    ["Scalability", "Integrations"][seed % 2],
                    ["High", "Very High", "Moderate"][seed % 3],
                    10 + seed % 10, 70 + seed % 20,
                    ["Ease of setup", "Affordable pricing"][seed % 2],
                    ["Self-serve tools", "Quick start guides"][seed % 2],
                    60 + seed % 30,
                    ["Advanced reporting", "Custom fields", "Workflow automation", "Mobile app", "API improvements"][seed % 5], 80 + seed % 50, ["In Progress", "Planned", "Under Review"][seed % 3],
                    ["Bulk import/export", "Real-time sync", "Template library", "Dark mode", "Keyboard shortcuts"][seed % 5], 60 + seed % 40, ["Planned", "In Progress", "Completed"][seed % 3],
                    ["Dashboard customization", "Role permissions", "Audit logs", "Webhooks", "Scheduled reports"][seed % 5], 50 + seed % 35, ["Under Review", "Planned", "In Progress"][seed % 3],
                    ["Quick actions", "Search improvements", "Data export", "Notifications", "Tagging"][seed % 5], 40 + seed % 30, ["Planned", "Under Review", "Backlog"][seed % 3],
                    ["SSO/SAML", "Advanced analytics", "White labeling", "Priority support", "Custom integrations"][seed % 5], 35 + seed % 25, ["In Progress", "Planned", "Under Review"][seed % 3],
                    ["Competitor A", "Competitor B", "Competitor C"][seed % 3], 15 + seed % 20, 20 + seed % 15,
                    ["Competitor B", "Competitor A", "Competitor D"][seed % 3], 10 + seed % 15, ["pricing", "mobile app", "integrations"][seed % 3],
                    ["Competitor C", "Competitor D", "Competitor A"][seed % 3], 8 + seed % 12,
                    ["Ship mobile improvements", "Enhance onboarding", "Improve documentation"][seed % 3],
                    ["mobile", "first-time user", "developer"][seed % 3],
                    ["Build custom analytics", "Add bulk operations", "Expand integrations"][seed % 3],
                    ["power user", "admin", "team"][seed % 3],
                    ["Develop enterprise features", "Create partner program", "Build marketplace"][seed % 3],
                    ["enterprise", "partner", "ecosystem"][seed % 3],
                    date, 10 + seed % 10, ["product", "engineering", "design"][seed % 3], date,
                    person, date, date,
                )
            )
        }
        _ => {
            (
                format!("analysis/{}/metrics-{}.md", project_slug, date),
                format!("Performance metrics analysis for {}", date),
                format!(
r#"# Performance Metrics - {} - {}

## Daily Summary
- **Total Sessions**: {}
- **Unique Users**: {}
- **Conversion Rate**: {}%
- **Revenue**: ${}

## Hourly Breakdown
| Hour | Sessions | Conversions | Revenue |
|------|----------|-------------|---------|
| 00-04 | {} | {} | ${} |
| 04-08 | {} | {} | ${} |
| 08-12 | {} | {} | ${} |
| 12-16 | {} | {} | ${} |
| 16-20 | {} | {} | ${} |
| 20-24 | {} | {} | ${} |

## Top Performing Segments
1. {} - {}% conversion, ${} revenue
2. {} - {}% conversion, ${} revenue
3. {} - {}% conversion, ${} revenue

## Anomalies Detected
- {}: {} at {} - {} from baseline

## Recommendations
Based on today's data:
1. {} shows opportunity for optimization
2. Consider {} for {} segment
3. Monitor {} closely tomorrow
"#,
                    project, date,
                    5000 + seed % 10000, 3000 + seed % 5000,
                    2.0 + (seed % 50) as f64 / 10.0, 5000 + seed % 20000,
                    100 + seed % 200, 2 + seed % 10, 100 + seed % 500,
                    300 + seed % 400, 8 + seed % 20, 400 + seed % 800,
                    1500 + seed % 2000, 40 + seed % 60, 2000 + seed % 3000,
                    1800 + seed % 2500, 50 + seed % 70, 2500 + seed % 4000,
                    1200 + seed % 1800, 35 + seed % 50, 1800 + seed % 2500,
                    600 + seed % 800, 15 + seed % 30, 800 + seed % 1500,
                    ["Enterprise", "Growth", "Starter"][seed % 3], 8 + seed % 10, 8000 + seed % 5000,
                    ["Returning users", "Mobile", "Referral"][seed % 3], 5 + seed % 8, 3000 + seed % 3000,
                    ["Direct traffic", "Email", "Organic"][seed % 3], 3 + seed % 5, 1500 + seed % 2000,
                    ["Conversion spike", "Traffic drop", "High bounce rate"][seed % 3],
                    ["+45%", "-30%", "+60%"][seed % 3], "14:30",
                    ["above", "below", "above"][seed % 3],
                    ["Checkout flow", "Landing page", "Pricing page"][seed % 3],
                    ["A/B test", "personalization", "retargeting"][seed % 3],
                    ["mobile", "enterprise", "new user"][seed % 3],
                    ["conversion rate", "bounce rate", "session duration"][seed % 3],
                )
            )
        }
    }
}

fn generate_technical_artifact(
    day: i64,
    idx: i64,
    seed: usize,
    project: &str,
    project_slug: &str,
    date: &str,
    week: i64,
) -> (String, String, String) {
    match (day + idx) % 4 {
        0 => (
            format!("docs/{}/architecture.md", project_slug),
            format!("Architecture documentation for {}", project),
            format!(
                r#"# {} - Architecture Documentation

## Overview
This document describes the technical architecture of the {} system, including component design, data flow, and integration patterns.

## System Components

### Core Services
```
┌─────────────────────────────────────────────────────────────┐
│                      Load Balancer                          │
│                    (AWS ALB / nginx)                        │
└─────────────────────┬───────────────────────────────────────┘
                      │
         ┌────────────┼────────────┐
         ▼            ▼            ▼
┌─────────────┐ ┌─────────────┐ ┌─────────────┐
│   API       │ │   Worker    │ │   Admin     │
│   Gateway   │ │   Service   │ │   Portal    │
│   (Go)      │ │   (Python)  │ │   (React)   │
└──────┬──────┘ └──────┬──────┘ └──────┬──────┘
       │               │               │
       └───────────────┴───────────────┘
                       │
         ┌─────────────┼─────────────┐
         ▼             ▼             ▼
┌─────────────┐ ┌─────────────┐ ┌─────────────┐
│  PostgreSQL │ │    Redis    │ │     S3      │
│  (Primary)  │ │   (Cache)   │ │  (Storage)  │
└─────────────┘ └─────────────┘ └─────────────┘
```

### API Gateway
- **Technology**: Go 1.21 with Chi router
- **Responsibilities**: Request routing, authentication, rate limiting
- **Scaling**: Horizontal, 2-10 instances based on load
- **Endpoints**: {} active routes across {} versions

### Worker Service
- **Technology**: Python 3.11 with Celery
- **Responsibilities**: Async job processing, scheduled tasks
- **Queue**: Redis-backed with {} concurrent workers
- **Job Types**: {}, {}, {}

### Data Layer
- **Primary DB**: PostgreSQL 15 ({} tables, {} indexes)
- **Caching**: Redis 7 ({}GB allocated)
- **Object Storage**: S3 ({} buckets)

## Data Flow

### Request Processing
1. Client request → Load Balancer
2. Route to API Gateway instance
3. JWT validation + rate limit check
4. Business logic execution
5. Database query (with Redis cache check)
6. Response serialization
7. Return to client

### Background Processing
1. API enqueues job to Redis
2. Worker picks up from queue
3. Process with retries (max {})
4. Store results in PostgreSQL
5. Notify via webhook if configured

## Security Architecture

### Authentication
- JWT tokens with {} expiry
- Refresh token rotation
- API key support for service accounts

### Authorization
- RBAC with {} roles
- Resource-level permissions
- Audit logging enabled

### Data Protection
- Encryption at rest (AES-256)
- TLS 1.3 in transit
- PII fields encrypted at application level

## Deployment

### Infrastructure
- **Cloud**: AWS (us-east-1, eu-west-1)
- **Orchestration**: Kubernetes (EKS)
- **CI/CD**: GitHub Actions → ArgoCD

### Environments
| Environment | Purpose | Instances |
|-------------|---------|-----------|
| Development | Feature work | 1 |
| Staging | Integration testing | 2 |
| Production | Live traffic | {} |

## Monitoring

### Observability Stack
- **Metrics**: Prometheus + Grafana
- **Logs**: Datadog
- **Traces**: Jaeger
- **Alerts**: PagerDuty integration

### Key Metrics
- Request latency (p50: {}ms, p99: {}ms)
- Error rate (target: <{}%)
- Queue depth
- Database connections

## Disaster Recovery

- **RPO**: {} minutes
- **RTO**: {} minutes
- **Backup Frequency**: Every {} hours
- **Multi-region**: Active-passive

---
*Last updated: {} | Version: {}.{}.{}*
"#,
                project,
                project,
                45 + seed % 30,
                2 + seed % 3,
                4 + seed % 8,
                ["email_send", "data_export", "report_generation"][seed % 3],
                ["notification", "sync", "cleanup"][seed % 3],
                ["analytics", "backup", "indexing"][seed % 3],
                30 + seed % 50,
                80 + seed % 100,
                2 + seed % 6,
                3 + seed % 5,
                3,
                ["1 hour", "24 hours", "15 minutes"][seed % 3],
                5 + seed % 10,
                4 + seed % 8,
                15 + seed % 10,
                150 + seed % 200,
                1 + seed % 2,
                5 + seed % 10,
                30 + seed % 60,
                6 + seed % 18,
                date,
                2,
                seed % 10,
                seed % 50,
            ),
        ),
        1 => (
            format!("docs/{}/runbook.md", project_slug),
            format!("Operations runbook for {}", project),
            format!(
                r#"# {} - Operations Runbook

## Quick Reference

### Service URLs
| Environment | URL | Health Check |
|-------------|-----|--------------|
| Production | https://api.{}.com | /health |
| Staging | https://staging.{}.com | /health |
| Development | https://dev.{}.com | /health |

### Key Contacts
- On-call: Check PagerDuty schedule
- Escalation: #ops-escalation Slack channel
- Database: DBA team (dba@company.com)

## Common Procedures

### Deployment
```bash
# Deploy to staging
kubectl set image deployment/{} {}={}:$VERSION -n staging

# Verify rollout
kubectl rollout status deployment/{} -n staging

# Deploy to production (requires approval)
./scripts/deploy-prod.sh $VERSION
```

### Scaling
```bash
# Scale API pods
kubectl scale deployment/{}-api --replicas={} -n production

# Scale workers
kubectl scale deployment/{}-worker --replicas={} -n production
```

### Database Operations
```bash
# Connect to production (read-only)
psql $PROD_DB_URL_RO

# Run migrations
./scripts/migrate.sh production

# Create backup
pg_dump $PROD_DB_URL > backup_$(date +%Y%m%d).sql
```

## Incident Response

### High Latency (>{}ms p99)
1. Check Grafana dashboard: {}
2. Verify database connection pool
3. Check Redis memory usage
4. Review recent deployments
5. Scale if needed: `kubectl scale deployment/{}-api --replicas={}`

### Error Rate Spike (>{}%)
1. Check error logs in Datadog
2. Identify affected endpoints
3. Check downstream dependencies
4. Review recent code changes
5. Consider rollback if needed

### Database Connection Issues
1. Check connection count: `SELECT count(*) FROM pg_stat_activity`
2. Kill idle connections if needed
3. Restart affected pods
4. Check for long-running queries

### Queue Backup
1. Check Redis memory: `redis-cli INFO memory`
2. Review job failure rate
3. Scale workers: `kubectl scale deployment/{}-worker --replicas={}`
4. Check for poison messages

## Health Checks

### API Health
```bash
curl -f https://api.{}.com/health || echo "UNHEALTHY"
```

### Worker Health
```bash
kubectl exec -it deploy/{}-worker -- celery inspect ping
```

### Database Health
```bash
psql $PROD_DB_URL -c "SELECT 1" || echo "DB UNHEALTHY"
```

## Rollback Procedures

### Application Rollback
```bash
# Get previous version
kubectl rollout history deployment/{}

# Rollback to previous
kubectl rollout undo deployment/{}

# Rollback to specific version
kubectl rollout undo deployment/{} --to-revision=N
```

### Database Rollback
```bash
# Check migration history
./scripts/migrate.sh history

# Rollback last migration
./scripts/migrate.sh rollback 1
```

## Monitoring Dashboards
- Main dashboard: https://grafana.internal/d/{}
- API metrics: https://grafana.internal/d/{}-api
- Worker metrics: https://grafana.internal/d/{}-worker
- Database: https://grafana.internal/d/{}-db

## Maintenance Windows
- Scheduled: Sundays 02:00-04:00 UTC
- Notify: #engineering channel 24h in advance
- Procedure: See maintenance playbook

---
*Last updated: {} | On-call: Check PagerDuty*
"#,
                project,
                project_slug,
                project_slug,
                project_slug,
                project_slug,
                project_slug,
                project_slug,
                project_slug,
                project_slug,
                5 + seed % 10,
                project_slug,
                3 + seed % 5,
                200 + seed % 300,
                project_slug,
                project_slug,
                8 + seed % 12,
                3 + seed % 5,
                project_slug,
                6 + seed % 10,
                project_slug,
                project_slug,
                project_slug,
                project_slug,
                project_slug,
                project_slug,
                project_slug,
                project_slug,
                project_slug,
                date,
            ),
        ),
        2 => (
            format!("docs/{}/api-reference.md", project_slug),
            format!("API reference documentation for {}", project),
            format!(
                r#"# {} - API Reference

## Base URL
```
Production: https://api.{}.com/v1
Staging: https://staging-api.{}.com/v1
```

## Authentication
All requests require a Bearer token in the Authorization header:
```
Authorization: Bearer <your_api_token>
```

## Rate Limits
- Standard: {} requests/minute
- Enterprise: {} requests/minute
- Burst: {} requests/second

## Endpoints

### Resources

#### List Resources
```http
GET /resources
```

**Query Parameters**
| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| page | integer | No | Page number (default: 1) |
| limit | integer | No | Items per page (default: 20, max: 100) |
| sort | string | No | Sort field (created_at, updated_at, name) |
| order | string | No | Sort order (asc, desc) |
| filter | string | No | Filter expression |

**Response**
```json
{{
  "data": [
    {{
      "id": "res_{}",
      "name": "Example Resource",
      "status": "active",
      "created_at": "{}T10:00:00Z",
      "updated_at": "{}T10:00:00Z",
      "metadata": {{
        "key": "value"
      }}
    }}
  ],
  "pagination": {{
    "page": 1,
    "limit": 20,
    "total": {},
    "pages": {}
  }}
}}
```

#### Get Resource
```http
GET /resources/:id
```

**Response**: Single resource object

#### Create Resource
```http
POST /resources
```

**Request Body**
```json
{{
  "name": "New Resource",
  "type": "standard",
  "config": {{
    "enabled": true,
    "threshold": {}
  }}
}}
```

**Response**: Created resource object with 201 status

#### Update Resource
```http
PATCH /resources/:id
```

**Request Body**: Partial resource object

#### Delete Resource
```http
DELETE /resources/:id
```

**Response**: 204 No Content

### Actions

#### Process Resource
```http
POST /resources/:id/process
```

**Request Body**
```json
{{
  "action": "{}",
  "options": {{
    "async": true,
    "notify": true
  }}
}}
```

**Response**
```json
{{
  "job_id": "job_{}",
  "status": "queued",
  "estimated_completion": "{}T10:05:00Z"
}}
```

### Webhooks

#### List Webhooks
```http
GET /webhooks
```

#### Create Webhook
```http
POST /webhooks
```

**Request Body**
```json
{{
  "url": "https://your-server.com/webhook",
  "events": ["resource.created", "resource.updated"],
  "secret": "your_webhook_secret"
}}
```

## Error Responses

### Error Format
```json
{{
  "error": {{
    "code": "RESOURCE_NOT_FOUND",
    "message": "Resource with ID res_123 not found",
    "details": {{}}
  }}
}}
```

### Common Error Codes
| Code | HTTP Status | Description |
|------|-------------|-------------|
| UNAUTHORIZED | 401 | Invalid or missing token |
| FORBIDDEN | 403 | Insufficient permissions |
| NOT_FOUND | 404 | Resource not found |
| RATE_LIMITED | 429 | Rate limit exceeded |
| VALIDATION_ERROR | 400 | Invalid request body |
| SERVER_ERROR | 500 | Internal server error |

## SDKs
- Python: `pip install {}-sdk`
- Node.js: `npm install @{}/sdk`
- Go: `go get github.com/{}/sdk-go`

## Changelog
- v1.{}: Added batch operations
- v1.{}: Improved filtering
- v1.{}: Initial release

---
*API Version: 1.{} | Generated: {}*
"#,
                project,
                project_slug,
                project_slug,
                100 + seed % 400,
                1000 + seed % 4000,
                10 + seed % 40,
                seed,
                date,
                date,
                100 + seed % 500,
                5 + seed % 20,
                50 + seed % 100,
                ["transform", "analyze", "export"][seed % 3],
                seed,
                date,
                project_slug,
                project_slug,
                project_slug,
                3 + seed % 5,
                2 + seed % 3,
                1,
                seed % 10,
                date,
            ),
        ),
        _ => (
            format!("reports/{}/sprint-{}.md", project_slug, week),
            format!("Sprint {} report for {}", week, project),
            format!(
                r#"# {} - Sprint {} Report

## Sprint Summary
- **Duration**: {} - {}
- **Velocity**: {} story points (target: {})
- **Completion Rate**: {}%

## Completed Stories

### Features
| ID | Title | Points | Owner |
|----|-------|--------|-------|
| {}-{} | {} | {} | {} |
| {}-{} | {} | {} | {} |
| {}-{} | {} | {} | {} |

### Bug Fixes
| ID | Title | Severity | Owner |
|----|-------|----------|-------|
| {}-{} | {} | {} | {} |
| {}-{} | {} | {} | {} |

### Technical Debt
| ID | Title | Points | Impact |
|----|-------|--------|--------|
| {}-{} | {} | {} | {} |

## Metrics

### Code Quality
- Test coverage: {}%
- Code review turnaround: {} hours avg
- Bugs found in review: {}

### Performance
- Build time: {} minutes
- Deploy time: {} minutes
- Test suite: {} minutes

## Retrospective Notes

### What Went Well
- {}
- {}
- {}

### What Could Improve
- {}
- {}

### Action Items
- [ ] {}
- [ ] {}

## Next Sprint Preview
- {} stories planned ({} points)
- Focus areas: {}, {}

---
*Sprint {} completed {} | Scrum Master: {}*
"#,
                project,
                week,
                date,
                date,
                25 + seed % 20,
                30,
                70 + seed % 30,
                project_slug.to_uppercase(),
                100 + seed,
                [
                    "Implement user dashboard",
                    "Add export functionality",
                    "Create admin panel"
                ][seed % 3],
                5 + seed % 8,
                PEOPLE[seed % PEOPLE.len()],
                project_slug.to_uppercase(),
                101 + seed,
                [
                    "Update notification system",
                    "Improve search performance",
                    "Add batch operations"
                ][seed % 3],
                3 + seed % 5,
                PEOPLE[(seed + 1) % PEOPLE.len()],
                project_slug.to_uppercase(),
                102 + seed,
                [
                    "Integrate with third-party API",
                    "Build reporting module",
                    "Enhance mobile view"
                ][seed % 3],
                8 + seed % 5,
                PEOPLE[(seed + 2) % PEOPLE.len()],
                project_slug.to_uppercase(),
                200 + seed,
                [
                    "Fix login timeout issue",
                    "Resolve data sync bug",
                    "Fix pagination error"
                ][seed % 3],
                ["Critical", "High", "Medium"][seed % 3],
                PEOPLE[(seed + 3) % PEOPLE.len()],
                project_slug.to_uppercase(),
                201 + seed,
                [
                    "Correct date formatting",
                    "Fix email template",
                    "Resolve cache issue"
                ][seed % 3],
                ["Medium", "Low", "High"][seed % 3],
                PEOPLE[(seed + 4) % PEOPLE.len()],
                project_slug.to_uppercase(),
                300 + seed,
                [
                    "Upgrade database driver",
                    "Refactor auth module",
                    "Update dependencies"
                ][seed % 3],
                3 + seed % 5,
                [
                    "Reduces tech debt",
                    "Improves security",
                    "Better performance"
                ][seed % 3],
                75 + seed % 20,
                2 + seed % 6,
                seed % 5,
                3 + seed % 5,
                5 + seed % 10,
                8 + seed % 12,
                [
                    "Team collaboration was excellent",
                    "Clear requirements helped delivery",
                    "Good testing practices"
                ][seed % 3],
                [
                    "Early identification of blockers",
                    "Effective async communication",
                    "Improved CI/CD pipeline"
                ][seed % 3],
                [
                    "Stakeholder demos went well",
                    "On-time delivery",
                    "Quality metrics improved"
                ][seed % 3],
                [
                    "Need more time for code review",
                    "Better estimation needed",
                    "More cross-team sync required"
                ][seed % 3],
                [
                    "Documentation could be improved",
                    "Testing coverage gaps",
                    "Technical debt accumulating"
                ][seed % 3],
                [
                    "Schedule estimation workshop",
                    "Set up documentation day",
                    "Create testing guidelines"
                ][seed % 3],
                [
                    "Improve standup efficiency",
                    "Add more automation",
                    "Better sprint planning"
                ][seed % 3],
                6 + seed % 6,
                30 + seed % 15,
                ["API improvements", "Performance", "Security"][seed % 3],
                ["UX polish", "Testing", "Documentation"][seed % 3],
                week,
                date,
                PEOPLE[seed % PEOPLE.len()],
            ),
        ),
    }
}

#[allow(clippy::format_in_format_args)]
fn generate_security_artifact(
    day: i64,
    idx: i64,
    seed: usize,
    date: &str,
) -> (String, String, String) {
    match (day + idx) % 3 {
        0 => (
            format!("security/audits/audit-{}.md", date),
            format!("Security audit report for {}", date),
            format!(
                r#"# Security Audit Report - {}

## Executive Summary
This report presents findings from the security assessment conducted on {}. The audit covered application security, infrastructure, and compliance controls.

**Overall Risk Level**: {}

## Scope
- Web application penetration testing
- API security assessment
- Infrastructure configuration review
- Access control audit
- Compliance verification (SOC 2, GDPR)

## Findings Summary

| Severity | Count | Fixed | Pending |
|----------|-------|-------|---------|
| Critical | {} | {} | {} |
| High | {} | {} | {} |
| Medium | {} | {} | {} |
| Low | {} | {} | {} |

## Critical Findings

### Finding 1: {}
- **Severity**: Critical
- **CVSS Score**: {}.{}
- **Status**: {}
- **Description**: {}
- **Impact**: {}
- **Remediation**: {}
- **Deadline**: {}

### Finding 2: {}
- **Severity**: High
- **CVSS Score**: {}.{}
- **Status**: {}
- **Description**: {}
- **Impact**: {}
- **Remediation**: {}

## Infrastructure Security

### Network Configuration
- Firewall rules: {} reviewed, {} issues found
- VPN configuration: {}
- Network segmentation: {}

### Cloud Security (AWS)
- IAM policies: {} roles reviewed
- S3 bucket permissions: {} public exposure risks
- Security groups: {} overly permissive rules

### Container Security
- Base images: {} outdated dependencies
- Secrets management: {}
- Runtime security: {}

## Application Security

### Authentication
- Password policy: {}
- MFA adoption: {}%
- Session management: {}

### Authorization
- RBAC implementation: {}
- Privilege escalation: {} potential issues
- API authorization: {}

### Data Protection
- Encryption at rest: {}
- Encryption in transit: {}
- PII handling: {}

## Compliance Status

### SOC 2
- Status: {}
- Last audit: {}
- Open items: {}

### GDPR
- Data processing agreements: {}
- Right to deletion: {}
- Consent management: {}

## Recommendations

### Immediate (0-30 days)
1. {}
2. {}
3. {}

### Short-term (30-90 days)
1. {}
2. {}

### Long-term (90+ days)
1. {}

## Appendix
- Detailed vulnerability scan results
- Network topology diagrams
- Remediation tracking spreadsheet

---
*Audit performed by: Security Team | Date: {}*
"#,
                date,
                date,
                ["Medium", "High", "Low"][seed % 3],
                seed % 2,
                seed % 2,
                0,
                2 + seed % 3,
                1 + seed % 2,
                1,
                5 + seed % 5,
                3 + seed % 3,
                2 + seed % 3,
                8 + seed % 8,
                6 + seed % 6,
                2 + seed % 4,
                [
                    "SQL Injection in Search API",
                    "Insecure Direct Object Reference",
                    "Authentication Bypass"
                ][seed % 3],
                8 + seed % 2,
                seed % 10,
                ["Fixed", "In Progress", "Pending"][seed % 3],
                [
                    "Parameterized queries not used in legacy endpoint",
                    "User IDs exposed in API responses",
                    "Token validation flaw"
                ][seed % 3],
                [
                    "Data breach risk",
                    "Unauthorized data access",
                    "Account takeover"
                ][seed % 3],
                [
                    "Implement parameterized queries",
                    "Add authorization checks",
                    "Fix token validation"
                ][seed % 3],
                date,
                [
                    "Sensitive Data Exposure",
                    "Cross-Site Scripting",
                    "Broken Access Control"
                ][seed % 3],
                7 + seed % 2,
                seed % 10,
                ["In Progress", "Pending", "Fixed"][seed % 3],
                [
                    "API returns sensitive fields",
                    "User input not sanitized",
                    "Missing function-level access control"
                ][seed % 3],
                [
                    "Information disclosure",
                    "Session hijacking",
                    "Privilege escalation"
                ][seed % 3],
                [
                    "Add response filtering",
                    "Implement input validation",
                    "Add authorization middleware"
                ][seed % 3],
                50 + seed % 30,
                seed % 5,
                ["Compliant", "Minor issues", "Needs review"][seed % 3],
                ["Properly configured", "Needs update", "Compliant"][seed % 3],
                20 + seed % 15,
                seed % 3,
                2 + seed % 5,
                3 + seed % 5,
                ["None found", "1 risk identified", "Addressed"][seed % 3],
                ["Enabled", "Partially enabled", "Needs implementation"][seed % 3],
                ["Strong", "Needs improvement", "Compliant"][seed % 3],
                70 + seed % 30,
                ["Compliant", "Minor gaps", "Needs work"][seed % 3],
                ["Properly implemented", "Gaps identified", "Compliant"][seed % 3],
                seed % 3,
                ["Properly configured", "Needs review", "Compliant"][seed % 3],
                ["Enabled", "Partial", "Full"][seed % 3],
                ["Enabled", "Full", "Compliant"][seed % 3],
                ["Compliant", "Needs review", "Properly handled"][seed % 3],
                ["Compliant", "In Progress", "Certified"][seed % 3],
                date,
                seed % 5,
                ["In place", "Needs update", "Compliant"][seed % 3],
                ["Implemented", "Needs testing", "Functional"][seed % 3],
                ["Properly configured", "Needs review", "Compliant"][seed % 3],
                [
                    "Patch critical vulnerabilities",
                    "Enable MFA for all admin accounts",
                    "Review IAM policies"
                ][seed % 3],
                [
                    "Update firewall rules",
                    "Implement rate limiting",
                    "Review access logs"
                ][seed % 3],
                [
                    "Rotate all API keys",
                    "Enable audit logging",
                    "Update security headers"
                ][seed % 3],
                ["Implement WAF", "Set up SIEM", "Conduct security training"][seed % 3],
                [
                    "Establish bug bounty program",
                    "Improve incident response",
                    "Update security policies"
                ][seed % 3],
                [
                    "Achieve SOC 2 Type II",
                    "Implement zero-trust architecture",
                    "Migrate to managed secrets service"
                ][seed % 3],
                date,
            ),
        ),
        _ => (
            format!("security/scans/vulnerability-{}.md", date),
            format!("Vulnerability scan results for {}", date),
            format!(
                r#"# Vulnerability Scan Report - {}

## Scan Information
- **Date**: {}
- **Scanner**: {}
- **Targets**: {} hosts, {} applications
- **Duration**: {} minutes

## Summary
| Severity | Count |
|----------|-------|
| Critical | {} |
| High | {} |
| Medium | {} |
| Low | {} |
| Info | {} |

## Critical Vulnerabilities

### CVE-{}-{}
- **CVSS**: 9.{}
- **Affected**: {}
- **Description**: {}
- **Remediation**: Apply patch version {}

## High Vulnerabilities

### CVE-{}-{}
- **CVSS**: {}.{}
- **Affected**: {}
- **Description**: {}

### CVE-{}-{}
- **CVSS**: {}.{}
- **Affected**: {}
- **Description**: {}

## Recommendations
1. Prioritize critical vulnerabilities
2. Schedule patching window
3. Verify fixes in staging first

---
*Next scan scheduled: {}*
"#,
                date,
                date,
                ["Qualys", "Nessus", "OpenVAS"][seed % 3],
                10 + seed % 20,
                3 + seed % 5,
                30 + seed % 60,
                seed % 3,
                2 + seed % 5,
                8 + seed % 10,
                15 + seed % 20,
                30 + seed % 50,
                2024,
                1000 + seed % 9000,
                seed % 10,
                ["web-server-01", "api-gateway", "database-primary"][seed % 3],
                [
                    "Remote code execution vulnerability",
                    "Authentication bypass",
                    "SQL injection"
                ][seed % 3],
                format!("{}.{}.{}", 2 + seed % 3, seed % 10, seed % 20),
                2024,
                2000 + seed % 8000,
                7 + seed % 2,
                seed % 10,
                ["load-balancer", "cache-server", "worker-node"][seed % 3],
                [
                    "Denial of service vulnerability",
                    "Information disclosure",
                    "Cross-site scripting"
                ][seed % 3],
                2024,
                3000 + seed % 7000,
                7,
                seed % 10,
                ["app-server-02", "admin-portal", "api-v2"][seed % 3],
                [
                    "Privilege escalation",
                    "Insecure deserialization",
                    "Path traversal"
                ][seed % 3],
                date,
            ),
        ),
    }
}

#[allow(clippy::too_many_arguments, clippy::format_in_format_args)]
fn generate_general_artifact(
    day: i64,
    idx: i64,
    seed: usize,
    project: &str,
    project_slug: &str,
    date: &str,
    week: i64,
    person: &str,
) -> (String, String, String) {
    match (day + idx) % 5 {
        0 => (
            format!("notes/{}/standup-{}.md", project_slug, date),
            format!("Standup notes for {} on {}", project, date),
            format!(
                r#"# {} - Daily Standup - {}

## Team Updates

### {}
**Yesterday**: {}
**Today**: {}
**Blockers**: {}

### {}
**Yesterday**: {}
**Today**: {}
**Blockers**: {}

### {}
**Yesterday**: {}
**Today**: {}
**Blockers**: {}

## Discussion Points
- {}
- {}

## Action Items
- [ ] {} - {}
- [ ] {} - {}

## Parking Lot
- {} (to discuss in refinement)
- {} (needs PM input)

---
*Standup duration: {} minutes | Facilitator: {}*
"#,
                project,
                date,
                PEOPLE[seed % PEOPLE.len()],
                [
                    "Completed feature implementation",
                    "Fixed critical bug",
                    "Finished code review"
                ][seed % 3],
                [
                    "Continue with testing",
                    "Start new user story",
                    "Deploy to staging"
                ][seed % 3],
                ["None", "Waiting for design", "Need API clarification"][seed % 3],
                PEOPLE[(seed + 1) % PEOPLE.len()],
                [
                    "Reviewed PRs",
                    "Updated documentation",
                    "Investigated production issue"
                ][seed % 3],
                [
                    "Finish documentation",
                    "Start refactoring",
                    "Help with testing"
                ][seed % 3],
                ["None", "Database access needed", "Unclear requirements"][seed % 3],
                PEOPLE[(seed + 2) % PEOPLE.len()],
                [
                    "Set up monitoring",
                    "Wrote integration tests",
                    "Deployed to staging"
                ][seed % 3],
                [
                    "Production deployment",
                    "Performance testing",
                    "Security review"
                ][seed % 3],
                ["None", "Waiting for approval", "Infrastructure issue"][seed % 3],
                [
                    "Sprint goal on track",
                    "Dependency on external team",
                    "Demo scheduled for Friday"
                ][seed % 3],
                [
                    "New requirements from stakeholder",
                    "Technical approach decision needed",
                    "Resource planning"
                ][seed % 3],
                "Follow up on API design",
                PEOPLE[(seed + 3) % PEOPLE.len()],
                "Schedule architecture review",
                PEOPLE[(seed + 4) % PEOPLE.len()],
                "Edge case handling in export feature",
                "Mobile app priority",
                10 + seed % 10,
                person,
            ),
        ),
        1 => (
            format!("projects/{}/README.md", project_slug),
            format!("Project README for {}", project),
            format!(
                r#"# {}

## Overview
{} is a {} designed to {}. It provides {} capabilities for {} use cases.

## Quick Start

### Prerequisites
- {} {}+
- {} {}+
- {} (optional)

### Installation
```bash
# Clone the repository
git clone https://github.com/company/{}

# Install dependencies
{} install

# Set up environment
cp .env.example .env

# Run database migrations
{} migrate

# Start the application
{} start
```

## Features
- **{}**: {}
- **{}**: {}
- **{}**: {}
- **{}**: {}

## Architecture
```
{}
├── src/
│   ├── api/          # API endpoints
│   ├── services/     # Business logic
│   ├── models/       # Data models
│   └── utils/        # Utilities
├── tests/            # Test suites
├── docs/             # Documentation
└── scripts/          # Automation scripts
```

## Configuration

### Environment Variables
| Variable | Description | Default |
|----------|-------------|---------|
| {} | {} | {} |
| {} | {} | {} |
| {} | {} | {} |

## Development

### Running Tests
```bash
{} test
{} test:coverage
```

### Code Style
- Follow {} style guide
- Run `{} lint` before committing
- Use conventional commits

## Deployment
See [deployment guide](./docs/deployment.md) for detailed instructions.

## Contributing
1. Fork the repository
2. Create a feature branch
3. Make your changes
4. Submit a pull request

## License
{}

## Contact
- Team: {}
- Slack: #{}
- Email: {}@company.com

---
*Last updated: {} | Version: {}.{}.{}*
"#,
                project,
                project,
                ["platform", "service", "application", "system"][seed % 4],
                [
                    "streamline workflows",
                    "improve productivity",
                    "enable collaboration",
                    "automate processes"
                ][seed % 4],
                ["robust", "scalable", "enterprise-grade", "modern"][seed % 4],
                ["business", "developer", "team", "enterprise"][seed % 4],
                ["Node.js", "Python", "Go", "Rust"][seed % 4],
                ["18", "3.11", "1.21", "1.70"][seed % 4],
                ["PostgreSQL", "MongoDB", "MySQL", "Redis"][seed % 4],
                ["15", "6", "8", "7"][seed % 4],
                ["Docker", "Kubernetes", "Terraform", "AWS CLI"][seed % 4],
                project_slug,
                ["npm", "pip", "go mod", "cargo"][seed % 4],
                ["npm run", "python manage.py", "go run", "cargo run"][seed % 4],
                ["npm run", "python manage.py", "go run", "cargo run"][seed % 4],
                ["Authentication", "Dashboard", "API Gateway", "Reporting"][seed % 4],
                [
                    "Secure login with MFA",
                    "Real-time analytics",
                    "Rate limiting and routing",
                    "Custom report builder"
                ][seed % 4],
                [
                    "User Management",
                    "Integrations",
                    "Notifications",
                    "Workflow Engine"
                ][seed % 4],
                [
                    "Role-based access control",
                    "Connect with 50+ services",
                    "Multi-channel alerts",
                    "Automated task execution"
                ][seed % 4],
                ["Data Export", "Audit Logs", "Search", "Collaboration"][seed % 4],
                [
                    "Export to CSV, JSON, PDF",
                    "Complete activity history",
                    "Full-text search",
                    "Team workspaces"
                ][seed % 4],
                ["Webhooks", "API", "Mobile", "Custom Fields"][seed % 4],
                [
                    "Real-time event notifications",
                    "RESTful API access",
                    "Native mobile apps",
                    "Extend with custom data"
                ][seed % 4],
                project_slug,
                "DATABASE_URL",
                "PostgreSQL connection string",
                "localhost:5432",
                "REDIS_URL",
                "Redis connection string",
                "localhost:6379",
                "API_KEY",
                "External API key",
                "required",
                ["npm run", "pytest", "go test", "cargo"][seed % 4],
                [
                    "npm run",
                    "pytest --cov",
                    "go test -cover",
                    "cargo tarpaulin"
                ][seed % 4],
                ["Airbnb", "PEP 8", "Go", "Rust"][seed % 4],
                ["npm run", "flake8", "golint", "cargo"][seed % 4],
                ["MIT", "Apache 2.0", "BSD-3", "ISC"][seed % 4],
                ["Engineering", "Platform", "Product", "DevOps"][seed % 4],
                project_slug,
                project_slug,
                date,
                1,
                seed % 10,
                seed % 50,
            ),
        ),
        2 => (
            format!("reports/{}/weekly-{}.md", project_slug, week),
            format!("Weekly report for {} - Week {}", project, week),
            format!(
                r#"# {} - Weekly Report - Week {}

## Summary
This week focused on {} with {}% completion of planned work. Key achievements include {} and {}.

## Completed Work

### Features
- [x] {} (PR #{})
- [x] {} (PR #{})
- [x] {} (PR #{})

### Bug Fixes
- [x] Fixed {} ({}-{})
- [x] Resolved {} ({}-{})

### Other
- [x] {}
- [x] {}

## In Progress
- [ ] {} - {}% complete
- [ ] {} - {}% complete
- [ ] {} - {}% complete

## Metrics

### Development
| Metric | This Week | Last Week | Change |
|--------|-----------|-----------|--------|
| PRs Merged | {} | {} | {} |
| Code Reviews | {} | {} | {} |
| Commits | {} | {} | {} |
| Test Coverage | {}% | {}% | {} |

### Performance
| Metric | Value | Target | Status |
|--------|-------|--------|--------|
| Uptime | {}% | 99.9% | {} |
| Avg Response | {}ms | <200ms | {} |
| Error Rate | {}% | <1% | {} |

## Team
- {} - Focused on {}, {}
- {} - Worked on {}, {}
- {} - Completed {}, {}

## Blockers
{}

## Next Week
1. {}
2. {}
3. {}

## Notes
{}

---
*Report by: {} | Week of: {}*
"#,
                project,
                week,
                [
                    "feature development",
                    "bug fixes",
                    "infrastructure",
                    "performance"
                ][seed % 4],
                75 + seed % 25,
                [
                    "new user dashboard",
                    "API v2 launch",
                    "mobile improvements",
                    "security hardening"
                ][seed % 4],
                [
                    "improved test coverage",
                    "documentation updates",
                    "performance gains",
                    "reduced tech debt"
                ][seed % 4],
                [
                    "User authentication flow",
                    "Data export feature",
                    "Admin dashboard",
                    "Search improvements"
                ][seed % 4],
                100 + seed,
                [
                    "Notification system",
                    "Batch operations",
                    "Reporting module",
                    "API rate limiting"
                ][seed % 4],
                101 + seed,
                [
                    "Mobile responsiveness",
                    "Dark mode support",
                    "Accessibility updates",
                    "Localization"
                ][seed % 4],
                102 + seed,
                ["login timeout issue", "data sync bug", "pagination error"][seed % 3],
                project_slug.to_uppercase(),
                200 + seed,
                ["caching problem", "email formatting", "timezone bug"][seed % 3],
                project_slug.to_uppercase(),
                201 + seed,
                [
                    "Updated documentation",
                    "Improved CI pipeline",
                    "Refactored test suite",
                    "Added monitoring"
                ][seed % 4],
                [
                    "Code review sessions",
                    "Knowledge sharing",
                    "Sprint planning",
                    "Retrospective"
                ][seed % 4],
                ["Advanced filtering", "Bulk import", "Custom webhooks"][seed % 3],
                60 + seed % 30,
                ["SSO integration", "Audit logging", "Role permissions"][seed % 3],
                40 + seed % 40,
                ["API pagination", "Error handling", "Caching layer"][seed % 3],
                20 + seed % 50,
                12 + seed % 8,
                10 + seed % 8,
                ["+2", "+1", "0", "-1"][seed % 4],
                25 + seed % 15,
                22 + seed % 15,
                ["+3", "+5", "-2", "0"][seed % 4],
                85 + seed % 50,
                80 + seed % 50,
                ["+5", "+10", "-3", "0"][seed % 4],
                78 + seed % 15,
                75 + seed % 15,
                ["+3%", "+1%", "-1%", "0%"][seed % 4],
                99.5 + (seed % 5) as f64 / 10.0,
                ["✓", "✓", "⚠"][seed % 3],
                120 + seed % 80,
                ["✓", "✓", "⚠"][seed % 3],
                (seed % 10) as f64 / 10.0,
                ["✓", "✓", "⚠"][seed % 3],
                PEOPLE[seed % PEOPLE.len()],
                ["features", "frontend", "API work"][seed % 3],
                ["testing", "documentation", "code review"][seed % 3],
                PEOPLE[(seed + 1) % PEOPLE.len()],
                ["backend", "database", "infrastructure"][seed % 3],
                ["bug fixes", "performance", "monitoring"][seed % 3],
                PEOPLE[(seed + 2) % PEOPLE.len()],
                ["integration", "security", "DevOps"][seed % 3],
                ["deployment", "automation", "testing"][seed % 3],
                [
                    "None this week",
                    "Waiting on design approval",
                    "External API dependency",
                    "Resource constraints"
                ][seed % 4],
                [
                    "Complete in-progress items",
                    "Start new sprint",
                    "Deploy to production"
                ][seed % 3],
                [
                    "Begin next feature set",
                    "Address tech debt",
                    "Improve documentation"
                ][seed % 3],
                ["Prepare for demo", "Team planning", "Stakeholder updates"][seed % 3],
                [
                    "Great progress this week",
                    "Team morale is high",
                    "On track for quarterly goals",
                    "Some challenges but manageable"
                ][seed % 4],
                person,
                date,
            ),
        ),
        3 => (
            format!("notes/{}/decisions-{}.md", project_slug, date),
            format!("Architecture decision record for {} on {}", project, date),
            format!(
                r#"# ADR: {} for {}

**Date**: {}
**Status**: {}
**Deciders**: {}, {}, {}

## Context
{}

## Problem Statement
We need to decide {} for the {} project. The current approach {} and we're evaluating alternatives to {}.

## Decision Drivers
- {}
- {}
- {}
- {}

## Considered Options

### Option 1: {}
**Pros**:
- {}
- {}
- {}

**Cons**:
- {}
- {}

**Estimated Effort**: {} weeks

### Option 2: {}
**Pros**:
- {}
- {}

**Cons**:
- {}
- {}
- {}

**Estimated Effort**: {} weeks

### Option 3: {}
**Pros**:
- {}
- {}

**Cons**:
- {}
- {}

**Estimated Effort**: {} weeks

## Decision
We chose **Option {}** because {}. This aligns with our goals of {} and {}.

## Consequences

### Positive
- {}
- {}
- {}

### Negative
- {}
- {}

### Risks
| Risk | Likelihood | Impact | Mitigation |
|------|------------|--------|------------|
| {} | {} | {} | {} |
| {} | {} | {} | {} |

## Implementation Plan
1. {} (Week 1)
2. {} (Week 2)
3. {} (Week 3-4)

## Metrics for Success
- {}
- {}
- {}

## Related Decisions
- [ADR-{}](./decisions-{}.md): {}
- [ADR-{}](./decisions-{}.md): {}

---
*Decision recorded by: {} | Approved: {}*
"#,
                [
                    "Database Selection",
                    "API Architecture",
                    "Authentication Strategy",
                    "Caching Approach"
                ][seed % 4],
                project,
                date,
                ["Accepted", "Proposed", "Approved"][seed % 3],
                PEOPLE[seed % PEOPLE.len()],
                PEOPLE[(seed + 1) % PEOPLE.len()],
                PEOPLE[(seed + 2) % PEOPLE.len()],
                [
                    "The system needs to handle increasing load while maintaining low latency",
                    "We're experiencing performance issues with the current implementation",
                    "New requirements necessitate a change in our technical approach",
                    "The team has identified opportunities for architectural improvement"
                ][seed % 4],
                [
                    "the right technology stack",
                    "our data storage strategy",
                    "the integration approach",
                    "our deployment model"
                ][seed % 4],
                project,
                [
                    "has scalability limitations",
                    "is becoming a bottleneck",
                    "doesn't meet new requirements",
                    "has maintenance overhead"
                ][seed % 4],
                [
                    "improve performance",
                    "reduce complexity",
                    "increase reliability",
                    "enable new features"
                ][seed % 4],
                "Performance requirements (sub-200ms response time)",
                "Scalability to 10x current load",
                "Team expertise and learning curve",
                "Cost considerations and budget constraints",
                [
                    "PostgreSQL with read replicas",
                    "Microservices migration",
                    "OAuth 2.0 + OIDC",
                    "Redis cluster"
                ][seed % 4],
                "Mature, well-understood technology",
                "Team has existing expertise",
                "Strong community support",
                "Requires infrastructure changes",
                "Initial migration effort",
                2 + seed % 4,
                [
                    "MongoDB sharded cluster",
                    "Service mesh with Istio",
                    "Custom JWT implementation",
                    "Memcached"
                ][seed % 4],
                "Better horizontal scaling",
                "More flexible schema",
                "Team needs training",
                "Operational complexity",
                "Less familiar tooling",
                3 + seed % 4,
                [
                    "CockroachDB",
                    "Serverless functions",
                    "Third-party auth service",
                    "Application-level caching"
                ][seed % 4],
                "Managed service reduces ops burden",
                "Pay-per-use pricing",
                "Vendor lock-in concerns",
                "Less control over internals",
                1 + seed % 3,
                1 + seed % 3,
                [
                    "it best balances our technical and business requirements",
                    "the team can implement it efficiently",
                    "it provides the best long-term value"
                ][seed % 3],
                ["reliability", "scalability", "performance"][seed % 3],
                ["maintainability", "cost efficiency", "developer experience"][seed % 3],
                "Improved system performance",
                "Better scalability for future growth",
                "Reduced operational complexity",
                "Initial implementation effort required",
                "Team will need some ramp-up time",
                ["Migration issues", "Integration problems"][seed % 2],
                ["Medium", "Low"][seed % 2],
                ["High", "Medium"][seed % 2],
                ["Thorough testing", "Gradual rollout"][seed % 2],
                ["Performance regression", "Cost overrun"][seed % 2],
                ["Low", "Medium"][seed % 2],
                ["Medium", "High"][seed % 2],
                ["Monitoring and benchmarks", "Budget reviews"][seed % 2],
                "Set up development environment",
                "Implement core changes",
                "Testing and gradual rollout",
                "Response time under 200ms at p95",
                "Zero downtime during migration",
                "Cost within 10% of budget",
                seed,
                date,
                "Previous architecture decision",
                seed + 1,
                date,
                "Related infrastructure choice",
                PEOPLE[seed % PEOPLE.len()],
                date,
            ),
        ),
        _ => (
            format!("notes/{}/meeting-{}.md", project_slug, date),
            format!("Meeting notes for {} on {}", project, date),
            format!(
                r#"# {} - Meeting Notes - {}

## Meeting Details
- **Type**: {}
- **Duration**: {} minutes
- **Attendees**: {}, {}, {}, {}

## Agenda
1. {}
2. {}
3. {}

## Discussion

### Topic 1: {}
{}

**Key Points**:
- {}
- {}
- {}

**Decision**: {}

### Topic 2: {}
{}

**Key Points**:
- {}
- {}

**Questions Raised**:
- {} - Answer: {}
- {} - Answer: {}

### Topic 3: {}
{}

**Outcome**: {}

## Action Items
| Item | Owner | Due Date | Status |
|------|-------|----------|--------|
| {} | {} | {} | {} |
| {} | {} | {} | {} |
| {} | {} | {} | {} |

## Decisions Made
1. {}
2. {}

## Next Steps
- {}
- {}
- {}

## Parking Lot
- {} (to be discussed in next meeting)
- {} (needs more research)

## Next Meeting
- **Date**: {} (tentative)
- **Topic**: {}

---
*Notes by: {} | Distributed to: team@company.com*
"#,
                project,
                date,
                [
                    "Sprint Planning",
                    "Retrospective",
                    "Technical Review",
                    "Stakeholder Sync"
                ][seed % 4],
                30 + seed % 60,
                PEOPLE[seed % PEOPLE.len()],
                PEOPLE[(seed + 1) % PEOPLE.len()],
                PEOPLE[(seed + 2) % PEOPLE.len()],
                PEOPLE[(seed + 3) % PEOPLE.len()],
                [
                    "Review last sprint",
                    "Discuss current blockers",
                    "Plan next iteration"
                ][seed % 3],
                [
                    "Technical deep-dive",
                    "Resource allocation",
                    "Timeline review"
                ][seed % 3],
                ["Open discussion", "Q&A", "Next steps"][seed % 3],
                [
                    "Project status update",
                    "Feature prioritization",
                    "Technical approach"
                ][seed % 3],
                [
                    "Discussed current progress and timeline for upcoming milestones.",
                    "Reviewed the backlog and identified top priorities for the next sprint.",
                    "Analyzed technical options and trade-offs for the proposed solution."
                ][seed % 3],
                "Progress is on track for the quarterly goal",
                "Some dependencies need to be resolved",
                "Team capacity looks good for planned work",
                [
                    "Proceed with proposed timeline",
                    "Prioritize the critical path items",
                    "Move forward with Option A"
                ][seed % 3],
                ["Resource planning", "Risk assessment", "Sprint velocity"][seed % 3],
                [
                    "Reviewed team capacity and identified potential conflicts.",
                    "Discussed potential risks and mitigation strategies.",
                    "Analyzed velocity trends and adjusted estimates."
                ][seed % 3],
                "Current allocation is sufficient",
                "Need to monitor closely",
                ["What's the impact on timeline?", "Can we parallelize work?"][seed % 2],
                ["Minimal with current plan", "Yes, with proper coordination"][seed % 2],
                ["Do we need external help?", "What are the dependencies?"][seed % 2],
                ["Not at this time", "Listed in the project plan"][seed % 2],
                [
                    "Next steps and action items",
                    "Open discussion",
                    "Feedback gathering"
                ][seed % 3],
                [
                    "Wrapped up with clear action items and owners.",
                    "Team provided feedback and suggestions for improvement.",
                    "Identified next steps and scheduled follow-up."
                ][seed % 3],
                [
                    "All agreed on the approach",
                    "Feedback incorporated into plan",
                    "Next steps defined"
                ][seed % 3],
                [
                    "Complete technical spec",
                    "Update project timeline",
                    "Review security requirements"
                ][seed % 3],
                PEOPLE[seed % PEOPLE.len()],
                date,
                ["Open", "In Progress", "Open"][seed % 3],
                [
                    "Schedule stakeholder demo",
                    "Finalize API design",
                    "Set up test environment"
                ][seed % 3],
                PEOPLE[(seed + 1) % PEOPLE.len()],
                date,
                ["Open", "Open", "In Progress"][seed % 3],
                [
                    "Document architecture",
                    "Create runbook",
                    "Update monitoring"
                ][seed % 3],
                PEOPLE[(seed + 2) % PEOPLE.len()],
                date,
                ["Open", "Open", "Open"][seed % 3],
                [
                    "Approved the proposed approach",
                    "Agreed to resource allocation",
                    "Confirmed timeline"
                ][seed % 3],
                [
                    "Set milestones for next phase",
                    "Established success metrics",
                    "Defined review cadence"
                ][seed % 3],
                "Continue with planned work",
                "Schedule follow-up meetings",
                "Share notes with stakeholders",
                "Long-term roadmap planning",
                "Integration with external system",
                date,
                ["Sprint review", "Technical deep-dive", "Planning session"][seed % 3],
                person,
            ),
        ),
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let workspace_path = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("./test-workspace"));

    log!("[Populate] === History Population Tool (with backdated timestamps) ===\n");
    log!("[Populate] Workspace: {}\n", workspace_path.display());

    // Create workspace structure
    std::fs::create_dir_all(workspace_path.join("data"))?;
    std::fs::create_dir_all(workspace_path.join("artifacts"))?;

    // Connect to PostgreSQL (use DATABASE_URL env var)
    let database_url = lucidos_engine::core::database_url();
    log!("[Populate] Connecting to PostgreSQL at {}", database_url);

    // Create shared connection pool
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(10)
        .connect(&database_url)
        .await?;

    // Initialize event store schema
    let event_store = EventStore::new(pool.clone());
    event_store.init_schema().await?;
    log!("[Populate] Event store initialized");

    // Initialize scheduler schemas (notifications)
    NotificationStore::init_schema(&pool).await?;
    log!("[Populate] Scheduler schemas initialized");

    // Initialize embedder and memory index
    log!("[Populate] Initializing embedder (first run downloads model ~30MB)...");
    let embedder = FastEmbedProvider::new()?;

    log!("[Populate] Creating memory index...");
    let memory_index = PgVectorIndex::new(pool.clone()).await?;

    let working_days: i64 = 730; // Full 2 years
    let conversations_per_day: i64 = 3;
    let artifacts_per_day: i64 = 1; // Fewer but more substantial artifacts

    // Base date: 2 years ago
    let base_date = Utc::now() - Duration::days(730);

    let total_events = working_days * (conversations_per_day * 2 + artifacts_per_day);
    log!(
        "\n[Populate] Populating {} events ({} days of history)...\n",
        total_events,
        working_days
    );

    let start = std::time::Instant::now();
    let mut event_count = 0;
    let mut notification_count = 0;
    let mut artifact_count = 0;
    let artifacts_path = workspace_path.join("artifacts");

    for day in 0..working_days {
        let project1 = PROJECTS[day as usize % PROJECTS.len()];
        let project2 = PROJECTS[(day as usize + 7) % PROJECTS.len()];
        let request_id = Uuid::new_v4();

        // Calculate backdated timestamp for this day
        // Add some variation within the day (9am-6pm)
        let day_start = base_date + Duration::days(day);
        let hour_offset = 9 + (day % 9); // 9am to 5pm
        let event_time = day_start + Duration::hours(hour_offset);

        // Simulate conversations
        for i in 0..conversations_per_day {
            let project = if i % 2 == 0 { project1 } else { project2 };
            let msg_time = event_time + Duration::minutes(i * 30);

            // User message
            let user_msg = generate_user_message(day, i, project);
            let user_event = BackdatedEvent::user_message(request_id, &user_msg, msg_time);
            append_backdated_event(&pool, &user_event).await?;

            let embedding = embedder.embed(&user_msg).await?;
            memory_index
                .index_entry(
                    user_event.id,
                    &MemorySource::Event { id: user_event.id },
                    "General",
                    &user_msg,
                    0.5,
                    &[],
                    &embedding,
                    embedder.model_id(),
                    msg_time,
                    lucidos_engine::memory::EXTRACTOR_VERSION,
                )
                .await?;
            event_count += 1;

            // Assistant response (a few seconds later)
            let response_time = msg_time + Duration::seconds(5);
            let assistant_msg = generate_assistant_response(day, i, project);
            let assistant_event =
                BackdatedEvent::assistant_response(request_id, &assistant_msg, response_time);
            append_backdated_event(&pool, &assistant_event).await?;

            let embedding = embedder.embed(&assistant_msg).await?;
            memory_index
                .index_entry(
                    assistant_event.id,
                    &MemorySource::Event {
                        id: assistant_event.id,
                    },
                    "General",
                    &assistant_msg,
                    0.5,
                    &[],
                    &embedding,
                    embedder.model_id(),
                    response_time,
                    lucidos_engine::memory::EXTRACTOR_VERSION,
                )
                .await?;
            event_count += 1;
        }

        // Simulate artifact creation - actually create files on disk
        for i in 0..artifacts_per_day {
            let project = if i % 2 == 0 { project1 } else { project2 };
            let (path, description, content) = generate_artifact_with_content(day, i, project);
            let artifact_time = event_time + Duration::hours(2) + Duration::minutes(i * 15);

            // Create the actual file on disk
            let full_path = artifacts_path.join(&path);
            if let Some(parent) = full_path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(&full_path, &content)?;

            let artifact_event = BackdatedEvent::new(
                "ArtifactCreated",
                serde_json::json!({
                    "path": path,
                    "description": description,
                    "commit": format!("abc{:04x}{:02x}", day, i)
                }),
                artifact_time,
            );
            append_backdated_event(&pool, &artifact_event).await?;

            let summary = format!("**{}**: {}", path, description);
            let embedding = embedder.embed(&summary).await?;
            let commit = format!("abc{:04x}{:02x}", day, i);
            memory_index
                .index_entry(
                    artifact_event.id,
                    &MemorySource::Artifact {
                        path: path.clone(),
                        commit,
                    },
                    "General",
                    &summary,
                    0.5,
                    &[],
                    &embedding,
                    embedder.model_id(),
                    artifact_time,
                    lucidos_engine::memory::EXTRACTOR_VERSION,
                )
                .await?;
            event_count += 1;
            artifact_count += 1;
        }

        // Generate morning brief notification every day
        let morning_time = day_start + Duration::hours(8); // 8am
        let brief = generate_morning_brief(day);
        let task_id = Uuid::new_v4();

        // Create notification
        NotificationStore::insert_with_timestamp(
            &pool,
            "Morning Brief",
            &brief,
            Some(task_id),
            Some("morning-brief"),
            morning_time,
        )
        .await?;

        // Create event for the scheduled task
        let task_request_id = Uuid::new_v4();
        let task_event = BackdatedEvent::trigger_completed(
            task_request_id,
            task_id,
            "Morning Brief",
            &brief,
            morning_time,
        );
        append_backdated_event(&pool, &task_event).await?;

        // Index the scheduled task event in memory (as regular event)
        let embedding = embedder.embed(&brief).await?;
        memory_index
            .index_entry(
                task_event.id,
                &MemorySource::Event { id: task_event.id },
                "Morning Brief",
                &brief,
                0.5,
                &[],
                &embedding,
                embedder.model_id(),
                morning_time,
                lucidos_engine::memory::EXTRACTOR_VERSION,
            )
            .await?;

        notification_count += 1;

        if day % 100 == 0 {
            let pct = (day as f64 / working_days as f64) * 100.0;
            log!(
                "[Populate]   Day {}/{} ({:.0}%) - {} events, {} artifacts, {} notifications...",
                day,
                working_days,
                pct,
                event_count,
                artifact_count,
                notification_count
            );
        }
    }

    // Mark older notifications as read (keep last 10 as unread)
    log!("\n[Populate] Marking older notifications as read (keeping 10 unread)...");
    sqlx::query(
        r#"
        UPDATE notifications
        SET read = true
        WHERE id NOT IN (
            SELECT id FROM notifications
            ORDER BY created_at DESC
            LIMIT 10
        )
        "#,
    )
    .execute(&pool)
    .await?;

    // Create personal tracking files (always present)
    log!("[Populate] Creating personal tracking files...");
    std::fs::write(
        artifacts_path.join("todo.md"),
        "# Todo\n\n- [ ] Review API migration PR\n- [ ] Call dentist\n- [ ] Finish quarterly report\n- [x] Order new laptop charger\n- [ ] Update project documentation\n- [ ] Schedule team retrospective\n",
    )?;
    std::fs::write(
        artifacts_path.join("shopping.md"),
        "# Shopping List\n\n- Milk\n- Eggs\n- Bread\n- Coffee\n- Bananas\n- Chicken\n- Rice\n- Olive oil\n",
    )?;
    std::fs::write(
        artifacts_path.join("reminders.md"),
        "# Reminders\n\n- **Friday**: Car service at 10am\n- **Next week**: Dentist appointment\n- **March 15**: API Migration deadline\n- **End of month**: Quarterly review\n",
    )?;

    let elapsed = start.elapsed();

    // Verify counts
    let db_events: Vec<(i64,)> = sqlx::query_as("SELECT COUNT(*) FROM events")
        .fetch_all(&pool)
        .await?;
    let db_notifications: Vec<(i64,)> = sqlx::query_as("SELECT COUNT(*) FROM notifications")
        .fetch_all(&pool)
        .await?;

    log!("\n[Populate] === Done ===");
    log!("[Populate]   PostgreSQL events: {}", db_events[0].0);
    log!("[Populate]   PostgreSQL notifications: {}", db_notifications[0].0);
    log!(
        "[Populate]   Memory index entries: {}",
        memory_index.len().await.unwrap_or(0)
    );
    log!("[Populate]   Artifact files created: {}", artifact_count);
    log!("[Populate]   Time: {:.1}s", elapsed.as_secs_f64());
    log!("[Populate]   Workspace: {}", workspace_path.display());
    log!("\n[Populate] To test, run:");
    log!(
        "[Populate]   LUCIDOS_WORKSPACE={} cargo run -p lucidos-engine",
        workspace_path.display()
    );
    log!("\n[Populate] Then open http://localhost:3000 in your browser.");
    log!("[Populate] Click the bell icon to see notifications!");

    Ok(())
}
