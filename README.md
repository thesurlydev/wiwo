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

# Last week
wiwo events --user octocat --time 1w

# Last 2 months
wiwo events --user octocat --time 2m

# Last year
wiwo events --user octocat --time 1y
```

Time range format:
- `Xd`: X days (e.g., `30d` for 30 days)
- `Xw`: X weeks (e.g., `2w` for 2 weeks)
- `Xm`: X months (e.g., `1m` for 1 month)
- `Xy`: X years (e.g., `1y` for 1 year)

If no time range is specified, defaults to 30 days.

**Note**: Due to GitHub API limitations, only events from the last 90 days are available. If you specify a longer time range, only events from the most recent 90 days will be shown.

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
