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
        /// GitHub username (defaults to authenticated user if GH_TOKEN is set)
        #[arg(short, long)]
        user: Option<String>,
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

#[derive(Debug, Deserialize, Clone)]
struct Repository {
    name: String,
    #[serde(default)]
    html_url: String,
    private: Option<bool>,
    #[serde(default)]
    clone_url: String,
    #[serde(default)]
    fork: bool,
}

#[derive(Debug, Deserialize)]
struct RepositoryDetails {
    private: bool,
}

#[derive(Debug, Deserialize)]
struct AuthenticatedUser {
    login: String,
}

impl Repository {
    fn html_url(&self) -> String {
        if !self.html_url.is_empty() {
            self.html_url.clone()
        } else {
            format!("https://github.com/{}", self.name)
        }
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
        Commands::Events { user, time } => fetch_user_events(user.as_deref(), &time).await?,
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

async fn fetch_events_from_api(client: &reqwest::Client, headers: &HeaderMap, username: &str, cutoff_time: DateTime<Utc>) -> Result<Vec<Event>> {
    // Define endpoints - only use direct events since received_events will duplicate activity
    let mut endpoints = vec![
        format!("https://api.github.com/users/{}/events/public", username),
        format!("https://api.github.com/users/{}/events", username),
    ];

    // Remove private endpoint if no token
    if !headers.contains_key(reqwest::header::AUTHORIZATION) {
        endpoints.retain(|e| e.contains("/public"));
    }

    let mut all_events = Vec::new();

    for endpoint in endpoints {
        match fetch_events_from_endpoint(client, headers, &endpoint, cutoff_time).await {
            Ok(mut events) => all_events.append(&mut events),
            Err(e) => eprintln!("Warning: Failed to fetch events from {}: {}", endpoint, e),
        }
    }
    
    Ok(all_events)
}

async fn fetch_events_from_endpoint(client: &reqwest::Client, headers: &HeaderMap, endpoint: &str, cutoff_time: DateTime<Utc>) -> Result<Vec<Event>> {
    // GitHub limits pagination to 10 pages with 100 items per page
    let mut all_events = Vec::new();
    let mut page = 1;
    let max_pages = 10;

    loop {
        if page > max_pages {
            eprintln!("Note: Only showing first {} pages of events due to GitHub API limitations", max_pages);
            break;
        }
        let url = format!("{endpoint}?page={page}&per_page=100");
        let response = client
            .get(&url)
            .headers(headers.clone())
            .send()
            .await
            .context(format!("Failed to fetch events from {}", endpoint))?;

        // Check rate limits
        let remaining = response.headers()
            .get("x-ratelimit-remaining")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse::<i32>().ok())
            .unwrap_or(0);

        let reset_time = response.headers()
            .get("x-ratelimit-reset")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse::<i64>().ok())
            .map(|ts| DateTime::<Utc>::from_timestamp(ts, 0).unwrap_or_default())
            .unwrap_or_default();

        if remaining == 0 {
            let now = Utc::now();
            let wait_time = (reset_time - now).num_seconds().max(0) as u64;
            if wait_time < 3600 { // Only wait if less than an hour
                eprintln!("Rate limit reached. Waiting {} seconds...", wait_time);
                tokio::time::sleep(tokio::time::Duration::from_secs(wait_time + 1)).await;
                continue;
            } else {
                eprintln!("Rate limit reset time too far in future ({} seconds)", wait_time);
                break;
            }
        }

        if response.status() == reqwest::StatusCode::NOT_FOUND {
            break;
        }

        // Get the response text first
        let text = response.text().await
            .context(format!("Failed to get response text from {}", endpoint))?;

        // Check if we got an error response
        if let Ok(error) = serde_json::from_str::<serde_json::Value>(&text) {
            if let Some(message) = error.get("message").and_then(|m| m.as_str()) {
                if message.contains("rate limit") {
                    eprintln!("Rate limit exceeded. Waiting before continuing...");
                    tokio::time::sleep(tokio::time::Duration::from_secs(60)).await;
                    continue;
                } else {
                    eprintln!("API error: {}", message);
                    break;
                }
            }
        }

        // Try to parse as array first, then as single event
        let events: Vec<Event> = match serde_json::from_str(&text) {
            Ok(events) => events,
            Err(e) => {
                // If parsing as array fails, try parsing as single event
                match serde_json::from_str::<Event>(&text) {
                    Ok(event) => vec![event],
                    Err(_) => {
                        // Only show error if response isn't empty
                        if !text.trim().is_empty() {
                            eprintln!("Warning: Failed to parse response from {}: {}", endpoint, e);
                        }
                        break;
                    }
                }
            }
        };

        let mut should_break = false;

        if events.is_empty() {
            // If we get an empty page, check if we have any events before the cutoff
            // If we do, we can stop. If not, keep going as there might be a gap
            if page >= 30 { // Try up to 30 pages per endpoint to get more history
                should_break = true;
            }
        } else {
            // Check if we've reached the cutoff time
            let reached_cutoff = events.last().map_or(false, |last_event| {
                last_event.created_at < cutoff_time
            });

            // Add events to our collection
            all_events.extend(events);

            if reached_cutoff {
                should_break = true;
            }
        }

        if should_break {
            break;
        }

        page += 1;

        // Add a small delay between requests
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
    }

    Ok(all_events)
}

async fn get_authenticated_user(client: &reqwest::Client, headers: &HeaderMap) -> Result<Option<String>> {
    if let Some(_auth_header) = headers.get(reqwest::header::AUTHORIZATION) {
        let response = client
            .get("https://api.github.com/user")
            .headers(headers.clone())
            .send()
            .await?;

        if response.status().is_success() {
            let user = response.json::<AuthenticatedUser>().await?;
            return Ok(Some(user.login));
        }
    }
    Ok(None)
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

async fn fetch_user_repositories(client: &reqwest::Client, headers: &HeaderMap, username: &str) -> Result<Vec<Repository>> {
    let mut all_repos = Vec::new();
    let mut page = 1;

    loop {
        let url = format!("https://api.github.com/users/{}/repos?type=owner&page={}&per_page=100", username, page);
        let response = client
            .get(&url)
            .headers(headers.clone())
            .send()
            .await
            .context(format!("Failed to fetch repositories for {}", username))?;

        let repos: Vec<Repository> = response.json().await
            .context("Failed to parse repository response")?;

        if repos.is_empty() {
            break;
        }

        all_repos.extend(repos.into_iter().filter(|r| !r.fork));
        page += 1;
    }

    Ok(all_repos)
}

async fn get_git_history(repo_path: &str, since: DateTime<Utc>) -> Result<Vec<Event>> {
    let output = tokio::process::Command::new("git")
        .arg("-C")
        .arg(repo_path)
        .arg("log")
        .arg("--all")
        .arg("--date=iso-strict")
        .arg(format!("--since={}", since.format("%Y-%m-%d")))
        .arg("--pretty=format:%H%n%aI%n%s%n%aN")
        .output()
        .await
        .context("Failed to execute git log")?;

    let output_str = String::from_utf8_lossy(&output.stdout);
    let mut events = Vec::new();

    for chunk in output_str.split("\n\n") {
        let parts: Vec<_> = chunk.split('\n').collect();
        if parts.len() >= 4 {
            if let Ok(created_at) = DateTime::parse_from_rfc3339(parts[1]) {
                events.push(Event {
                    event_type: "Push".to_string(),
                    repo: Repository {
                        name: repo_path.to_string(),
                        html_url: String::new(),
                        private: None,
                        clone_url: String::new(),
                        fork: false,
                    },
                    created_at: created_at.with_timezone(&Utc),
                });
            }
        }
    }

    Ok(events)
}

async fn fetch_user_events(username_arg: Option<&str>, time_range: &str) -> Result<()> {
    let (client, headers) = setup_github_client()?;
    
    // If no username provided, try to get authenticated user
    let username = match username_arg {
        Some(name) => name.to_string(),
        None => {
            match get_authenticated_user(&client, &headers).await? {
                Some(user) => user,
                None => anyhow::bail!("No username provided and no authenticated user found. Please provide a username or set GH_TOKEN.")
            }
        }
    };
    
    // Create a cache for repository visibility
    let repo_cache = Arc::new(RwLock::new(HashMap::new()));

    let duration = parse_time_range(time_range)?;
    let requested_cutoff = Utc::now() - duration;
    
    // GitHub API only returns events from the last 90 days
    let max_duration = Duration::days(90);
    let api_cutoff = Utc::now() - max_duration;
    
    println!("
Fetching GitHub events for {} (since {})
", 
        username,
        requested_cutoff.format("%Y-%m-%d %H:%M:%S UTC")
    );
    
    // For events within 90 days, use the GitHub Events API
    let mut all_events = Vec::new();
    
    if duration <= max_duration {
        // If requested duration is within API limits, use that
        all_events.extend(fetch_events_from_api(&client, &headers, &username, requested_cutoff).await?);
    } else {
        // For recent events (last 90 days), use the API
        all_events.extend(fetch_events_from_api(&client, &headers, &username, api_cutoff).await?);
        
        // For older events, use git history
        eprintln!("Fetching older events from git history (this may take a while)...");
        
        // Create temp directory for cloning
        let temp_dir = tempfile::tempdir()?;
        
        // Get all repositories owned by the user
        let repos = fetch_user_repositories(&client, &headers, &username).await?;
        
        for repo in repos {
            let repo_path = temp_dir.path().join(&repo.name);
            
            // Clone repository
            let output = tokio::process::Command::new("git")
                .arg("clone")
                .arg("--no-checkout")
                .arg("--filter=tree:0")
                .arg(&repo.clone_url)
                .arg(&repo_path)
                .output()
                .await?;
                
            if output.status.success() {
                // Get git history
                let mut repo_events = get_git_history(repo_path.to_str().unwrap(), requested_cutoff).await?;
                
                // Update event details
                for event in &mut repo_events {
                    event.repo = repo.clone();
                }
                
                all_events.extend(repo_events);
            }
        }
    }

    // Remove duplicates based on created_at and event_type
    all_events.sort_by(|a, b| b.created_at.cmp(&a.created_at));
    all_events.dedup_by(|a, b| {
        a.created_at == b.created_at && 
        a.event_type == b.event_type && 
        a.repo.name == b.repo.name
    });

    if all_events.is_empty() {
        println!("No events found.");
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
