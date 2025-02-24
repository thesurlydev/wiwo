# WIWO (What I Worked On)

A command-line tool to list GitHub events for a given user.

## Installation

Ensure you have Rust installed, then:

```bash
cargo install --path .
```
This will install the `wiwo` binary in your `$HOME/.cargo/bin` directory.

## Usage

To list GitHub events for a user:

```bash
wiwo events [--user <github-username>] [--time <time-range>]
```

Examples:
```bash
# Using authenticated user (requires GH_TOKEN)
wiwo events

# Specific user, last 30 days (default)
wiwo events --user octocat

# Last 3 days
wiwo events --user octocat --time 3d

# Last 2 weeks
wiwo events --user octocat --time 2w

# Last 60 days
wiwo events --user octocat --time 60d

# Last 90 days (maximum supported by GitHub API)
wiwo events --user octocat --time 90d
```

Time range format:
- `Xd`: X days (e.g., `30d` for 30 days)
- `Xw`: X weeks (e.g., `2w` for 2 weeks)
- `Xm`: X months (e.g., `1m` for 1 month)

If no time range is specified, defaults to 30 days.

**Note**: The GitHub Events API only returns events from the last 90 days. For older events, `wiwo` will:
1. Use the Events API to fetch the most recent 90 days of activity
2. Clone all repositories owned by the user (using a temporary directory)
3. Use git history to find commits and other activity from before the 90-day limit

This means that for timeframes longer than 90 days:
- Initial fetching may take longer due to repository cloning
- Only events that leave a git history trace will be shown (commits, tags, etc.)
- Events like issue comments, watches, and follows won't be available beyond 90 days

### Authentication

To access private repositories, get better API rate limits, and use the authenticated user by default, you can set your GitHub token in the environment:

```bash
export GH_TOKEN=your_github_token_here
```

When a token is provided, the tool will:
- Use your GitHub account as the default user if --user is not specified
- Include events from private repositories
- Access additional event endpoints
- Have higher API rate limits
- Show events you've received from other users

### Notes on Event History

The tool attempts to fetch as much event history as possible, but there are some GitHub API limitations:

- Recent events (last 90 days) are fetched from the public events API
- For older events, the tool tries multiple API endpoints to gather more history
- Some events might not be available due to GitHub's event retention policies
- The number of API requests is rate-limited by GitHub

## Features

- Fetches and displays recent public GitHub activities
- Shows timestamp, event type, and repository for each activity
- Clean and readable output format
