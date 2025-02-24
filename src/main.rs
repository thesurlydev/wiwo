use anyhow::{Context, Result};
use clap::Parser;
use reqwest::header::{HeaderMap, HeaderValue, ACCEPT, USER_AGENT};
use serde::Deserialize;
use chrono::{DateTime, Utc, Duration};

#[derive(Parser)]
#[command(author, version, about, long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(clap::Subcommand)]
enum Commands {
    /// List GitHub events for a user
    Events {
        /// GitHub username
        #[arg(short, long)]
        user: String,
        /// Time range for events (e.g., "30d" for 30 days, "1m" for 1 month)
        #[arg(short, long, default_value = "30d")]
        time: String,
    },
}

#[derive(Debug, Deserialize)]
struct Event {
    #[serde(rename = "type")]
    event_type: String,
    created_at: DateTime<Utc>,
    repo: Repository,
}

impl Event {
    fn formatted_type(&self) -> String {
        // Remove the 'Event' suffix if present
        let event_type = self.event_type.strip_suffix("Event")
            .unwrap_or(&self.event_type);

        // Special cases for specific event types
        match event_type {
            "PullRequest" => "PR".to_string(),
            "PullRequestReview" => "PR Review".to_string(),
            "PullRequestReviewComment" => "PR Comment".to_string(),
            "IssueComment" => "Issue Cmt".to_string(),
            other => other.to_string(),
        }
    }
}

use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

#[derive(Debug, Deserialize)]
struct Repository {
    name: String,
}

#[derive(Debug, Deserialize)]
struct RepositoryDetails {
    private: bool,
}

impl Repository {
    fn html_url(&self) -> String {
        format!("https://github.com/{}", self.name)
    }

    async fn is_private(
        &self,
        client: &reqwest::Client,
        headers: &HeaderMap,
        cache: &Arc<RwLock<HashMap<String, bool>>>
    ) -> Result<bool> {
        // Check cache first
        if let Some(&is_private) = cache.read().await.get(&self.name) {
            return Ok(is_private);
        }

        // Make API call to get repository details
        let url = format!("https://api.github.com/repos/{}", self.name);
        match client
            .get(&url)
            .headers(headers.clone())
            .send()
            .await
        {
            Ok(response) => {
                if response.status() == reqwest::StatusCode::NOT_FOUND {
                    // Cache and return false for not found repositories
                    cache.write().await.insert(self.name.clone(), false);
                    return Ok(false);
                }

                match response.json::<RepositoryDetails>().await {
                    Ok(details) => {
                        // Cache the result
                        cache.write().await.insert(self.name.clone(), details.private);
                        Ok(details.private)
                    }
                    Err(_) => {
                        // Cache false on error
                        cache.write().await.insert(self.name.clone(), false);
                        Ok(false)
                    }
                }
            }
            Err(_) => {
                // Cache false on error
                cache.write().await.insert(self.name.clone(), false);
                Ok(false)
            }
        }
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Events { user, time } => fetch_user_events(&user, &time).await?,
    }

    Ok(())
}

fn pad_to_width(s: &str, width: usize) -> String {
    if s.len() >= width {
        s.to_string()
    } else {
        format!("{:width$}", s, width = width)
    }
}

fn parse_time_range(time_str: &str) -> Result<Duration> {
    let len = time_str.len();
    if len < 2 {
        anyhow::bail!("Invalid time format. Use format like '30d' for 30 days or '1m' for 1 month");
    }

    let (amount_str, unit) = time_str.split_at(len - 1);
    let amount: i64 = amount_str.parse()
        .context("Invalid number in time range")?;

    match unit {
        "d" => Ok(Duration::days(amount)),
        "m" => Ok(Duration::days(amount * 30)),  // Approximate month as 30 days
        "w" => Ok(Duration::weeks(amount)),
        "y" => Ok(Duration::days(amount * 365)), // Approximate year as 365 days
        _ => anyhow::bail!("Invalid time unit. Use 'd' for days, 'w' for weeks, 'm' for months, or 'y' for years")
    }
}

async fn fetch_activity_page(client: &reqwest::Client, headers: &HeaderMap, username: &str, page: u32) -> Result<Vec<Event>> {
    let url = format!(
        "https://api.github.com/users/{}/events?page={}&per_page=100",
        username, page
    );

    let response = client
        .get(&url)
        .headers(headers.clone())
        .send()
        .await
        .context("Failed to fetch GitHub events")?;

    if response.status() == reqwest::StatusCode::FORBIDDEN {
        anyhow::bail!("GitHub API rate limit exceeded. Please try again later.");
    }

    let events = response
        .json()
        .await
        .context("Failed to parse GitHub events")?;

    Ok(events)
}

fn setup_github_client() -> Result<(reqwest::Client, HeaderMap)> {
    let mut headers = HeaderMap::new();
    headers.insert(ACCEPT, HeaderValue::from_static("application/vnd.github.v3+json"));
    headers.insert(USER_AGENT, HeaderValue::from_static("wiwo-cli"));

    // Check for GitHub token in environment
    if let Ok(token) = std::env::var("GH_TOKEN") {
        headers.insert(
            reqwest::header::AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {}", token))
                .context("Invalid GitHub token format")?
        );
    }

    Ok((reqwest::Client::new(), headers))
}

async fn fetch_user_events(username: &str, time_range: &str) -> Result<()> {
    // Create a cache for repository visibility
    let repo_cache = Arc::new(RwLock::new(HashMap::new()));
    let (client, headers) = setup_github_client()?;

    let duration = parse_time_range(time_range)?;
    let cutoff_time = Utc::now() - duration;
    
    println!("\nFetching GitHub events for {} (since {})\n", 
        username,
        cutoff_time.format("%Y-%m-%d %H:%M:%S UTC")
    );

    let mut all_events = Vec::new();
    let mut page = 1;

    // Calculate the duration in days
    let days = match time_range.chars().last() {
        Some('d') => duration.num_days(),
        Some('w') => duration.num_weeks() * 7,
        Some('m') => duration.num_days(), // Already converted to days in parse_time_range
        Some('y') => duration.num_days(), // Already converted to days in parse_time_range
        _ => duration.num_days(),
    };

    // Determine which API endpoints to use based on time range
    let endpoints = if days <= 90 {
        // If we have a token, try the private events endpoint first
        if headers.contains_key(reqwest::header::AUTHORIZATION) {
            vec![
                format!("https://api.github.com/users/{}/events", username),
                format!("https://api.github.com/users/{}/events/public", username)
            ]
        } else {
            vec![format!("https://api.github.com/users/{}/events/public", username)]
        }
    } else {
        // For longer periods, try multiple event types
        // For longer time periods with token, try all available endpoints
        if headers.contains_key(reqwest::header::AUTHORIZATION) {
            vec![
                format!("https://api.github.com/users/{}/events", username),
                format!("https://api.github.com/users/{}/events/public", username),
                format!("https://api.github.com/users/{}/received_events", username)
            ]
        } else {
            vec![
                format!("https://api.github.com/users/{}/events/public", username),
                format!("https://api.github.com/users/{}/events", username)
            ]
        }
    };

    for endpoint in endpoints {
        let mut page = 1;
        loop {
            let url = format!("{}?page={}&per_page=100", endpoint, page);
            
            let response = client
                .get(&url)
                .headers(headers.clone())
                .send()
                .await
                .context(format!("Failed to fetch GitHub events from {}", endpoint))?;

            if response.status() == reqwest::StatusCode::FORBIDDEN {
                eprintln!("Rate limit reached for {}", endpoint);
                break;
            }

            if response.status() == reqwest::StatusCode::NOT_FOUND {
                break;
            }

            let events: Vec<Event> = match response.json().await {
                Ok(events) => events,
                Err(e) => {
                    eprintln!("Warning: Failed to parse events from {}: {}", endpoint, e);
                    break;
                }
            };

            if events.is_empty() {
                break;
            }

            // Check if the oldest event in this page is already too old
            if let Some(oldest) = events.last() {
                if oldest.created_at < cutoff_time {
                    all_events.extend(events.into_iter().filter(|e| e.created_at > cutoff_time));
                    break;
                }
            }

            all_events.extend(events);
            page += 1;

            if page > 30 { // Increased limit for better historical coverage
                break;
            }
        }
    }

    // Remove duplicates based on created_at and event_type
    all_events.sort_by(|a, b| {
        let date_cmp = b.created_at.cmp(&a.created_at);
        if date_cmp == std::cmp::Ordering::Equal {
            a.event_type.cmp(&b.event_type)
        } else {
            date_cmp
        }
    });

    all_events.dedup_by(|a, b| {
        a.created_at == b.created_at && 
        a.event_type == b.event_type && 
        a.repo.name == b.repo.name
    });

    // Sort events by date (newest first)
    all_events.sort_by(|a, b| b.created_at.cmp(&a.created_at));


    if all_events.is_empty() {
        println!("No recent events found.");
        return Ok(());
    }

    // Find the maximum widths for each column
    let max_type_width = all_events.iter()
        .map(|e| e.formatted_type().len())
        .max()
        .unwrap_or(0)
        .max(10); // Minimum width of 10 for event type

    let max_repo_width = all_events.iter()
        .map(|e| e.repo.name.len())
        .max()
        .unwrap_or(0)
        .max(10); // Minimum width of 10 for repo name

    // Print header
    println!("{} | {} | {} | {} | {}",
        pad_to_width("TIMESTAMP", 19),
        pad_to_width("EVENT", max_type_width),
        pad_to_width("REPOSITORY", max_repo_width),
        pad_to_width("VISIBILITY", 10),
        "URL"
    );
    println!("{}-+-{}-+-{}-+-{}-+-{}",
        "-".repeat(19),
        "-".repeat(max_type_width),
        "-".repeat(max_repo_width),
        "-".repeat(10),
        "-".repeat(20)
    );

    // Print events
    for event in all_events {
        let is_private = event.repo.is_private(&client, &headers, &repo_cache).await?;
        println!("{} | {} | {} | {} | {}",
            event.created_at.format("%Y-%m-%d %H:%M:%S"),
            pad_to_width(&event.formatted_type(), max_type_width),
            pad_to_width(&event.repo.name, max_repo_width),
            pad_to_width(if is_private { "Private" } else { "Public" }, 10),
            event.repo.html_url()
        );
    }

    Ok(())
}
