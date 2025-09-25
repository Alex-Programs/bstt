// src/main.rs

use chrono::{prelude::*, Duration};
use clap::Parser;
use comfy_table::{
    modifiers::UTF8_ROUND_CORNERS, presets::UTF8_FULL, Cell, Color, ContentArrangement, Table,
};
use colored::*;
use indicatif::{ProgressBar, ProgressStyle};
use serde::{Deserialize, Serialize};
use std::{error::Error, fs, path::Path, sync::Arc, thread};

// --- Configuration & Constants ---
const CONFIG_DIR: &str = "/etc/bstt";
const CONFIG_FILE: &str = "config.toml";

// --- Data Structures (FIXED) ---

#[derive(Serialize, Deserialize, Debug)]
struct Config {
    api: ApiConfig,
}

#[derive(Serialize, Deserialize, Debug)]
struct ApiConfig {
    cookie: String,
}

#[derive(Serialize, Deserialize, Debug)]
struct ApiResponse {
    events: Vec<Event>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
struct Event {
    #[serde(rename = "desc1")]
    title: String,
    #[serde(rename = "desc2")]
    event_type: String,
    start: String,
    end: String,
    #[serde(rename = "locAdd1")]
    location: String,
    // BUG FIX: Changed teacher_name to an Option to handle cases where it's missing from the API response.
    #[serde(rename = "teacherName")]
    teacher_name: Option<String>,
}

// --- CLI Argument Parsing ---

#[derive(Parser, Debug)]
#[command(author, version, about = "Fetches and displays University of Bristol student timetable.", long_about = None)]
struct Cli {
    /// Day offset from today for full timetable view. E.g., 0 for today, +1 for tomorrow.
    #[arg(default_value = "0")]
    day_offset: String,

    /// Enable compact, single-line output for status bars like Polybar
    #[arg(long)]
    mini: bool,
}

// --- Core Logic ---

fn load_or_create_config() -> Result<Config, Box<dyn Error + Send + Sync>> {
    let config_dir = Path::new(CONFIG_DIR);
    let config_path = config_dir.join(CONFIG_FILE);

    if !config_path.exists() {
        eprintln!("{} Config file not found at '{}'.", "Warning:".yellow(), config_path.display());
        if !config_dir.exists() {
            fs::create_dir_all(config_dir).map_err(|e| format!("Failed to create config directory at '{}': {}. Try `sudo mkdir -p {}`.", config_dir.display(), e, config_dir.display()))?;
        }
        let template = "[api]\ncookie = \"YourCookieHere\"\n";
        fs::write(&config_path, template).map_err(|e| format!("Failed to create config file at '{}': {}.", config_path.display(), e))?;
        eprintln!("A template config has been created. Edit it with your cookie: `sudo nano {}`", config_path.display());
        std::process::exit(1);
    }

    let config_str = fs::read_to_string(&config_path)?;
    let config: Config = toml::from_str(&config_str)?;

    if config.api.cookie == "YourCookieHere" {
        eprintln!("{} Your config at '{}' still contains the default value.", "Error:".red().bold(), config_path.display());
        eprintln!("Please replace 'YourCookieHere' with your actual cookie.");
        std::process::exit(1);
    }
    Ok(config)
}

// --- fetch_events (MODIFIED WITH BETTER ERROR HANDLING) ---
fn fetch_events(config: &Config) -> Result<ApiResponse, Box<dyn Error + Send + Sync>> {
    let today = Utc::now();
    let start_date = (today - Duration::days(90)).format("%Y-%m-%dT%H:%M:%S.000Z").to_string();
    let end_date = (today + Duration::days(90)).format("%Y-%m-%dT%H:%M:%S.000Z").to_string();
    
    let url = format!("https://app.bristol.ac.uk/campusm/sso/cal2/Student%20Timetable?start={}&end={}", start_date, end_date);

    let client = reqwest::blocking::Client::new();
    let response = client
        .get(url)
        .header("Cookie", &config.api.cookie)
        .header("User-Agent", "bstt/0.4.0 (Linux CLI Timetable Tool)")
        .header("Accept", "*/*")
        .header("Accept-Language", "en-US,en;q=0.5")
        .header("Referer", "https://app.bristol.ac.uk/campusm/home")
        .header("X-Requested-With", "XMLHttpRequest")
        .header("pragma", "no-cache")
        .header("cache-control", "no-cache")
        .send()?;
    
    let status = response.status();
    if !status.is_success() {
        let body = response.text().unwrap_or_else(|_| "Could not read response body".to_string());
        return Err(format!("API request failed with status: {}. Server response:\n{}", status, body).into());
    }

    // IMPROVED ERROR HANDLING: Read body as text first, then attempt to parse.
    // This allows us to include the problematic body in the error message.
    let body_text = response.text()?;
    let data: ApiResponse = serde_json::from_str(&body_text)
        .map_err(|e| {
            format!(
                "Failed to decode JSON response from server. Error: {}\n\n---\nReceived Body:\n{}---",
                e, body_text
            )
        })?;

    Ok(data)
}

// --- Full Timetable Display (FIXED) ---
fn display_timetable(events_data: ApiResponse, target_date: NaiveDate) {
    let mut daily_events: Vec<Event> = events_data.events.into_iter().filter(|event| {
        if let Ok(start_time) = DateTime::parse_from_rfc3339(&event.start) {
            start_time.with_timezone(&Local).date_naive() == target_date
        } else { false }
    }).collect();

    daily_events.sort_by(|a, b| a.start.cmp(&b.start));
    
    let date_str = target_date.format("%A, %d %B %Y").to_string();
    let day_diff = target_date.signed_duration_since(Local::now().date_naive()).num_days();
    let day_label = match day_diff { 0 => " (Today)", 1 => " (Tomorrow)", -1 => " (Yesterday)", _ => "" };
    
    println!(" {} {}{}", "Timetable for".bold(), date_str.bold(), day_label.bold());

    if daily_events.is_empty() {
        println!("\n{}", "No events scheduled for this day.".green());
        return;
    }

    let mut table = Table::new();
    table.load_preset(UTF8_FULL).apply_modifier(UTF8_ROUND_CORNERS).set_content_arrangement(ContentArrangement::Dynamic);
    
    table.set_header(vec![
        Cell::new("Time").fg(Color::Magenta), Cell::new("Type").fg(Color::Magenta),
        Cell::new("Event").fg(Color::Magenta), Cell::new("Location").fg(Color::Magenta),
        Cell::new("Lecturer").fg(Color::Magenta),
    ]);

    for event in daily_events {
        let start_time = DateTime::parse_from_rfc3339(&event.start).unwrap();
        let end_time = DateTime::parse_from_rfc3339(&event.end).unwrap();
        let time_str = format!("{} - {}", start_time.with_timezone(&Local).format("%H:%M"), end_time.with_timezone(&Local).format("%H:%M"));
        
        // BUG FIX: Gracefully handle the Option<String> for teacher_name.
        let main_lecturer = event.teacher_name
            .as_deref() // Convert Option<String> to Option<&str>
            .unwrap_or("") // Provide a default empty string if None
            .split(',')
            .next()
            .unwrap_or("")
            .trim();

        table.add_row(vec![
            Cell::new(time_str).fg(Color::Cyan), Cell::new(event.event_type).fg(Color::Yellow),
            Cell::new(event.title), Cell::new(event.location).fg(Color::Green),
            Cell::new(main_lecturer).fg(Color::Blue),
        ]);
    }
    println!("{}", table);
}

// --- Compression Helpers (Unchanged) ---
fn apply_transformations(mut s: String, rules: &[(&str, &str)]) -> String {
    for (find, replace) in rules.iter() {
        s = s.replace(find, replace);
    }
    s
}

fn compress_title(title: &str) -> String {
    let compound_rules = [
        ("Software Engineering", "SE"), ("Data Structures", "DS"), ("Intro to AI", "AI"),
        ("Practical Physics-Computing Lecture", "Labs-Comp Lec"), ("Practical Physics-Computing Drop-in", "Labs-Comp DI"),
        ("Probability & Statistics for Physicists", "Prob+Stats P"), ("Introductory Mathematics for Physics", "Intro M for P"),
        ("Intro to Coding and Data Analysis", "Coding+D.A."), ("Core Physics I Problem Class", "Core P PrbCls"),
        ("Intro Mathematics Examples Class", "Intro M ExCls"), ("Practical Physics", "Labs"), ("Problem Class", "PrbCls"),
    ];
    let atomic_rules = [
        ("Introductory", "Intro"), ("Introduction", "Intro"), ("Mathematics", "M"), ("Physics", "P"),
        ("Probability", "Prob"), ("Statistics", "Stats"), ("Computing", "Comp"),
        ("Lecture", "Lec"), ("Tutorial", "Tut"), ("Workshop", "W"), ("Project", "Proj"), ("Assembly", "Asmbly"),
    ];
    let symbol_rules = [(" and ", " + "), (" & ", " + "), (" for ", " "), (" of ", " "), (" to ", " ")];
    let mut processed_title = apply_transformations(title.to_string(), &compound_rules);
    processed_title = apply_transformations(processed_title, &atomic_rules);
    processed_title = apply_transformations(processed_title, &symbol_rules);
    let numerals = [" V", " IV", " III", " II", " I"];
    for num in numerals.iter() {
        if processed_title.ends_with(num) {
            processed_title = processed_title[..processed_title.len() - num.len()].to_string();
            break;
        }
    }
    let words: Vec<&str> = processed_title.split_whitespace().filter(|word| !word.to_lowercase().starts_with("grp")).collect();
    words.join(" ")
}

fn compress_location(location: &str) -> String {
    let rules = [
        ("Physics Building", "Phys"), ("Priory Road Complex", "PrioryRd"),
        ("Biomedical Sciences Building", "BioSci"), ("31-37 St. Michael's Hill", "StMichHill"),
        ("Queen's Building", "Queens"), ("Chemistry Building", "Chem"), ("Fry Building", "Fry"),
        ("Lecture Theatre", "LT"), ("Building", "Bldg"), ("Complex", "Cmplx"),
        (" Room", ""), ("Rear:", ""), (": ", ":"),
    ];
    apply_transformations(location.to_string(), &rules)
}

// --- Mini-Mode Display (MODIFIED) ---
fn display_mini_timetable(events_data: ApiResponse) {
    let now = Local::now();
    let today = now.date_naive();

    // Get all of today's events and sort them.
    let mut todays_events: Vec<Event> = events_data.events.into_iter().filter(|event| {
        if let Ok(start_time) = DateTime::parse_from_rfc3339(&event.start) {
            start_time.with_timezone(&Local).date_naive() == today
        } else { false }
    }).collect();
    todays_events.sort_by(|a, b| a.start.cmp(&b.start));

    // Find the current event.
    let current_event = todays_events.iter().find(|&event| {
        let start_time = DateTime::parse_from_rfc3339(&event.start).unwrap().with_timezone(&Local);
        let end_time = DateTime::parse_from_rfc3339(&event.end).unwrap().with_timezone(&Local);
        now >= start_time && now < end_time
    });

    // Find the next upcoming event.
    let next_event = todays_events.iter().find(|&event| {
        let start_time = DateTime::parse_from_rfc3339(&event.start).unwrap().with_timezone(&Local);
        start_time > now
    });

    if let Some(current) = current_event {
        // A class is currently in progress.
        let end_time = DateTime::parse_from_rfc3339(&current.end).unwrap().with_timezone(&Local);
        let border_time = end_time - Duration::minutes(10);
        
        // Check if we are in the 10-minute "border" window before the end.
        if now >= border_time {
            if let Some(next) = next_event {
                // We are in the border and there is another class today.
                let current_end_str = end_time.format("%H:%M");
                let next_start_str = DateTime::parse_from_rfc3339(&next.start).unwrap().with_timezone(&Local).format("%H:%M");
                let next_title = compress_title(&next.title);
                let next_loc = compress_location(&next.location);
                print!("BRD {}→{} | {} @ {}", current_end_str, next_start_str, next_title, next_loc);
            } else {
                // In the border, but it's the last class of the day. Treat as a normal current class.
                let current_title = compress_title(&current.title);
                let current_loc = compress_location(&current.location);
                print!("CUR {} | {} END {}", current_title, current_loc, end_time.format("%H:%M"));
            }
        } else {
            // Not in the border window yet. Just show the current class.
            let current_title = compress_title(&current.title);
            let current_loc = compress_location(&current.location);
            print!("CUR {} | {} END {}", current_title, current_loc, end_time.format("%H:%M"));
        }
    } else if let Some(next) = next_event {
        // No current class, but there is a next one today.
        let next_title = compress_title(&next.title);
        let next_loc = compress_location(&next.location);
        let next_start = DateTime::parse_from_rfc3339(&next.start).unwrap().with_timezone(&Local);
        print!("NXT {} | {} @ {}", next_title, next_loc, next_start.format("%H:%M"));
    } else {
        // No current or upcoming classes for the rest of the day.
        print!("TTB: BLK");
    }
}


// --- Main Execution ---
fn run() -> Result<(), Box<dyn Error + Send + Sync>> {
    let cli = Cli::parse();
    let config = load_or_create_config()?;
    let spinner = ProgressBar::new_spinner();
    spinner.set_style(ProgressStyle::default_spinner().tick_strings(&["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"]).template("{spinner:.blue} {msg}")?);
    if !cli.mini { spinner.set_message("Fetching timetable..."); }
    let config_clone = Arc::new(config);
    let handle = thread::spawn(move || fetch_events(&config_clone));
    if !cli.mini {
        while !handle.is_finished() {
            spinner.tick();
            thread::sleep(std::time::Duration::from_millis(50));
        }
    }
    let all_events = match handle.join().unwrap() {
        Ok(events) => {
            if !cli.mini { spinner.finish_with_message("✓".green().to_string()); }
            events
        },
        Err(e) => {
            if !cli.mini { spinner.finish_with_message("✗".red().to_string()); }
            if cli.mini { print!("TTB: ERR"); return Ok(()); }
            return Err(e);
        }
    };
    if cli.mini {
        display_mini_timetable(all_events);
    } else {
        let offset: i64 = cli.day_offset.parse().map_err(|_| "Invalid day offset.")?;
        let target_date = Local::now().date_naive() + Duration::days(offset);
        display_timetable(all_events, target_date);
    }
    Ok(())
}

fn main() {
    if let Err(e) = run() {
        eprintln!("{} {}", "Error:".red().bold(), e);
        std::process::exit(1);
    }
}