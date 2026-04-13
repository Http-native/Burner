use crate::remote::{self, Client, DeployRequest};
use crate::runtime::{LocalManager, Manager, default_manager, manager_for, run_command_foreground};
use crate::service::{Definition, normalize_location, timestamp, validate_name};
use crate::store::{Link, Store};
use crate::ui;
use anyhow::{Result, anyhow, bail};
use rand::RngCore;
use std::env;

pub fn run(args: Vec<String>) -> Result<()> {
    if args.is_empty() {
        bail!(usage_error(""));
    }

    let root = Store::default_root()?;
    let st = Store::new(root);
    st.init()?;

    match args[0].as_str() {
        "online" | "daemon" => run_online(&st, &args[1..]),
        "serve" => run_serve(&args[1..]),
        "link" => run_link(&st, &args[1..]),
        "deploy" => run_deploy(&st, &args[1..]),
        "run" => run_run(&st, &args[1..]),
        "list" => run_list_with_args(&st, &args[1..]),
        "logs" => run_logs(&st, &args[1..]),
        "start" => run_control(&st, "start", &args[1..]),
        "stop" => run_control(&st, "stop", &args[1..]),
        "restart" => run_control(&st, "restart", &args[1..]),
        "help" | "-h" | "--help" => bail!(usage_error("")),
        other => bail!(usage_error(&format!("unknown command \"{other}\""))),
    }
}

fn run_deploy(st: &Store, args: &[String]) -> Result<()> {
    let (mut name, flag_args) = split_leading_name(args)?;

    let mut command = String::new();
    let mut location = String::new();
    let mut server_id = String::new();
    let mut include_files = false;
    let rest = parse_flags(
        flag_args,
        &mut [
            FlagValue::new("-c", &mut command),
            FlagValue::new("-l", &mut location),
            FlagValue::new("-s", &mut server_id),
        ],
        &mut [BoolFlag::new("-file", &mut include_files)],
    )?;

    if name.is_empty() && rest.len() == 1 {
        name = rest[0].clone();
    }
    if name.is_empty() || rest.len() > 1 {
        bail!("usage: burner deploy <service-name> -c \"<command>\" -l \"<location>\"");
    }
    if command.is_empty() {
        bail!("deploy requires -c with the command to run");
    }

    validate_name(&name)?;

    let mut location_value = if location.is_empty() {
        String::new()
    } else {
        normalize_location(&location)?
    };

    let mut def = Definition {
        name: name.clone(),
        command: command.clone(),
        location: location_value.clone(),
        status: "pending".into(),
        ..Definition::default()
    };

    if !server_id.is_empty() {
        let link = st.get_link(&server_id)?;

        if location.is_empty() {
            location_value = default_deploy_folder()?;
            def.location = location_value.clone();
            include_files = true;
        }

        let mut req = DeployRequest {
            name: name.clone(),
            command: command.clone(),
            location: location_value.clone(),
            include_files: false,
            archive: String::new(),
        };
        if include_files {
            req.include_files = true;
            req.archive = remote::encode_directory_base64(&location_value)?;
        }

        Client::new().deploy(&link, &req)?;
        ui::print_success(&format!("deployed {} to {}", ui::colorize_name(&name), link.id));
        return Ok(());
    }

    if location_value.is_empty() {
        location_value = normalize_location(".")?;
        def.location = location_value;
    }

    let manager = default_manager();
    def.runtime = manager.name().into();
    let exec_path = env::current_exe()?.to_string_lossy().into_owned();
    manager.deploy(&mut def, Some(&exec_path))?;
    st.save(&mut def)?;
    ui::print_success(&format!(
        "deployed {} using {}",
        ui::colorize_name(&def.name),
        ui::colorize_runtime(&def.runtime)
    ));
    Ok(())
}

fn run_run(st: &Store, args: &[String]) -> Result<()> {
    let mut command = String::new();
    let mut location = String::new();
    let mut name = String::new();
    let mut service_name = String::new();
    let mut foreground = false;

    let rest = parse_flags(
        args,
        &mut [
            FlagValue::new("-c", &mut command),
            FlagValue::new("-l", &mut location),
            FlagValue::new("--name", &mut name),
            FlagValue::new("--service", &mut service_name),
        ],
        &mut [BoolFlag::new("--foreground", &mut foreground)],
    )?;
    if !rest.is_empty() {
        bail!("run requires -c with the command to run");
    }

    if !service_name.is_empty() {
        let def = st.get(&service_name)?;
        return run_foreground(def, foreground);
    }

    if command.is_empty() {
        bail!("run requires -c with the command to run");
    }

    let location_value = normalize_location(&location)?;
    let service_value = if name.is_empty() {
        format!("run-{}", chrono::Utc::now().format("%Y%m%d-%H%M%S"))
    } else {
        name
    };
    validate_name(&service_value)?;

    let mut def = Definition {
        name: service_value,
        command,
        location: location_value,
        status: "pending".into(),
        ..Definition::default()
    };

    if foreground {
        return run_foreground(def, true);
    }

    let manager = LocalManager;
    manager.start(&mut def)?;
    st.save(&mut def)?;
    ui::print_success(&format!(
        "started {} with pid {}",
        ui::colorize_name(&def.name),
        ui::colorize_pid(&def.pid.to_string())
    ));
    Ok(())
}

fn run_foreground(def: Definition, force: bool) -> Result<()> {
    if !force {
        bail!("stored service execution requires --foreground");
    }
    run_command_foreground(&def.location, &def.command)
}

fn run_list_with_args(st: &Store, args: &[String]) -> Result<()> {
    let mut server_id = String::new();
    let rest = parse_flags(
        args,
        &mut [FlagValue::new("-s", &mut server_id)],
        &mut [],
    )?;
    if !rest.is_empty() {
        bail!(usage_error(""));
    }

    if !server_id.is_empty() {
        let link = st.get_link(&server_id)?;
        let services = Client::new().list(&link)?;
        return print_services(&services);
    }

    let mut services = st.list()?;
    if services.is_empty() {
        ui::print_muted("No services deployed yet.");
        return Ok(());
    }

    for service in &mut services {
        if let Ok(manager) = manager_for(service) {
            if let Ok(status) = manager.status(service) {
                service.status = status;
                let mut saved = service.clone();
                let _ = st.save(&mut saved);
            }
        }
    }
    print_services(&services)
}

fn print_services(services: &[Definition]) -> Result<()> {
    if services.is_empty() {
        ui::print_muted("No services deployed yet.");
        return Ok(());
    }

    let rows: Vec<[String; 5]> = services
        .iter()
        .map(|def| {
            [
                def.name.clone(),
                value_or_dash(&def.runtime),
                value_or_dash(&def.status),
                def.pid.to_string(),
                def.location.clone(),
            ]
        })
        .collect();

    let headers = ["NAME", "RUNTIME", "STATUS", "PID", "LOCATION"];
    let mut widths = headers.map(str::len);
    for row in &rows {
        for (index, value) in row.iter().enumerate() {
            widths[index] = widths[index].max(value.len());
        }
    }

    let border = table_border(&widths);
    println!("{}", ui::colorize_border(&border));
    println!(
        "{} {} {} {} {} {} {} {} {} {} {}",
        ui::colorize_border("|"),
        ui::pad_right(ui::colorize_header(headers[0]), headers[0].len(), widths[0]),
        ui::colorize_border("|"),
        ui::pad_right(ui::colorize_header(headers[1]), headers[1].len(), widths[1]),
        ui::colorize_border("|"),
        ui::pad_right(ui::colorize_header(headers[2]), headers[2].len(), widths[2]),
        ui::colorize_border("|"),
        ui::pad_right(ui::colorize_header(headers[3]), headers[3].len(), widths[3]),
        ui::colorize_border("|"),
        ui::pad_right(ui::colorize_header(headers[4]), headers[4].len(), widths[4]),
        ui::colorize_border("|"),
    );
    println!("{}", ui::colorize_border(&border));
    for row in rows {
        println!(
            "{} {} {} {} {} {} {} {} {} {} {}",
            ui::colorize_border("|"),
            ui::pad_right(ui::colorize_name(&row[0]), row[0].len(), widths[0]),
            ui::colorize_border("|"),
            ui::pad_right(ui::colorize_runtime(&row[1]), row[1].len(), widths[1]),
            ui::colorize_border("|"),
            ui::pad_right(ui::colorize_status(&row[2]), row[2].len(), widths[2]),
            ui::colorize_border("|"),
            ui::pad_right(ui::colorize_pid(&row[3]), row[3].len(), widths[3]),
            ui::colorize_border("|"),
            ui::pad_right(ui::colorize_location(&row[4]), row[4].len(), widths[4]),
            ui::colorize_border("|"),
        );
    }
    println!("{}", ui::colorize_border(&border));
    Ok(())
}

fn table_border(widths: &[usize; 5]) -> String {
    let mut line = String::new();
    for width in widths {
        line.push('+');
        line.push_str(&"-".repeat(*width + 2));
    }
    line.push('+');
    line
}

fn run_control(st: &Store, action: &str, args: &[String]) -> Result<()> {
    let (mut name, flag_args) = split_leading_name(args)?;
    let mut server_id = String::new();
    let rest = parse_flags(
        flag_args,
        &mut [FlagValue::new("-s", &mut server_id)],
        &mut [],
    )?;

    if name.is_empty() && rest.len() == 1 {
        name = rest[0].clone();
    }
    if name.is_empty() || rest.len() > 1 {
        bail!("usage: burner {action} <service-name>");
    }

    if !server_id.is_empty() {
        let link = st.get_link(&server_id)?;
        Client::new().control(&link, action, &name)?;
        ui::print_success(&format!(
            "{} {} on {}",
            action,
            ui::colorize_name(&name),
            link.id
        ));
        return Ok(());
    }

    let mut def = st.get(&name)?;
    let manager = manager_for(&def)?;
    match action {
        "start" => manager.start(&mut def)?,
        "stop" => manager.stop(&mut def)?,
        "restart" => manager.restart(&mut def)?,
        _ => bail!("unsupported action \"{action}\""),
    }
    st.save(&mut def)?;
    ui::print_success(&format!("{} {}", action, ui::colorize_name(&def.name)));
    Ok(())
}

fn run_logs(st: &Store, args: &[String]) -> Result<()> {
    let (mut name, flag_args) = split_leading_name(args)?;
    let mut lines = String::from("100");
    let mut server_id = String::new();
    let rest = parse_flags(
        flag_args,
        &mut [
            FlagValue::new("-n", &mut lines),
            FlagValue::new("-s", &mut server_id),
        ],
        &mut [],
    )?;

    if name.is_empty() && rest.len() == 1 {
        name = rest[0].clone();
    }
    if name.is_empty() || rest.len() > 1 {
        bail!("usage: burner logs <service-name> -n <line-count>");
    }
    let lines: usize = lines.parse().unwrap_or(100);

    if !server_id.is_empty() {
        let link = st.get_link(&server_id)?;
        let output = Client::new().logs(&link, &name, lines)?;
        print!("{}", String::from_utf8_lossy(&output));
        return Ok(());
    }

    let def = st.get(&name)?;
    let manager = manager_for(&def)?;
    let output = manager.logs(&def, lines)?;
    print!("{}", String::from_utf8_lossy(&output));
    Ok(())
}

fn run_online(st: &Store, args: &[String]) -> Result<()> {
    let mut port = String::new();
    let rest = parse_flags(args, &mut [FlagValue::new("-p", &mut port)], &mut [])?;
    if !rest.is_empty() || port.is_empty() {
        bail!("usage: burner online -p <port>");
    }
    let port: u16 = port.parse().map_err(|_| anyhow!("usage: burner online -p <port>"))?;

    let name = online_service_name(port);
    let api_key = st.ensure_api_key()?;
    let location_value = normalize_location(".")?;
    let exec_path = env::current_exe()?.to_string_lossy().into_owned();
    let command = format!("{exec_path:?} serve -p {port}");

    if let Ok(mut def) = st.get(&name) {
        def.command = command;
        def.location = location_value;
        let manager = manager_for(&def)?;
        manager.restart(&mut def)?;
        st.save(&mut def)?;
        ui::print_success(&format!("daemon listening on {}", ui::colorize_pid(&format!(":{port}"))));
        ui::print_info(&format!(
            "service {} using {}",
            ui::colorize_name(&def.name),
            ui::colorize_runtime(&def.runtime)
        ));
        ui::print_info(&format!("api key {}", ui::colorize_secret(&api_key)));
        ui::print_muted(&format!(
            "Link with: burner link -url \"http://server\" -p {port} -k \"{api_key}\""
        ));
        return Ok(());
    }

    let mut def = Definition {
        name,
        command,
        location: location_value,
        status: "pending".into(),
        ..Definition::default()
    };

    let manager = default_manager();
    def.runtime = manager.name().into();
    manager.deploy(&mut def, Some(&exec_path))?;
    st.save(&mut def)?;
    ui::print_success(&format!("daemon listening on {}", ui::colorize_pid(&format!(":{port}"))));
    ui::print_info(&format!(
        "service {} using {}",
        ui::colorize_name(&def.name),
        ui::colorize_runtime(&def.runtime)
    ));
    ui::print_info(&format!("api key {}", ui::colorize_secret(&api_key)));
    ui::print_muted(&format!(
        "Link with: burner link -url \"http://server\" -p {port} -k \"{api_key}\""
    ));
    Ok(())
}

fn run_serve(args: &[String]) -> Result<()> {
    let mut port = String::new();
    let rest = parse_flags(args, &mut [FlagValue::new("-p", &mut port)], &mut [])?;
    if !rest.is_empty() || port.is_empty() {
        bail!("usage: burner serve -p <port>");
    }
    let port: u16 = port.parse().map_err(|_| anyhow!("usage: burner serve -p <port>"))?;
    ui::print_info(&format!("serving remote API on {}", ui::colorize_pid(&format!(":{port}"))));
    remote::serve(&format!("0.0.0.0:{port}"))
}

fn run_link(st: &Store, args: &[String]) -> Result<()> {
    let mut raw_url = String::new();
    let mut port = String::new();
    let mut api_key = String::new();
    let rest = parse_flags(
        args,
        &mut [
            FlagValue::new("-url", &mut raw_url),
            FlagValue::new("-p", &mut port),
            FlagValue::new("-k", &mut api_key),
        ],
        &mut [],
    )?;
    if !rest.is_empty() || raw_url.is_empty() || port.is_empty() || api_key.is_empty() {
        bail!("usage: burner link -url \"http://server\" -p <port> -k \"<api-key>\"");
    }
    let port: u16 = port
        .parse()
        .map_err(|_| anyhow!("usage: burner link -url \"http://server\" -p <port> -k \"<api-key>\""))?;
    let base_url = remote::normalize_base_url(&raw_url, port)?;
    let link = Link {
        id: random_id(),
        url: raw_url,
        port,
        base_url,
        api_key,
        created_at: timestamp(),
    };

    Client::new().ping(&link)?;
    st.save_link(&link)?;
    ui::print_success(&format!("linked {} as {}", link.base_url, link.id));
    Ok(())
}

fn split_leading_name(args: &[String]) -> Result<(String, &[String])> {
    if args.is_empty() {
        return Ok((String::new(), args));
    }
    if args[0].starts_with('-') {
        return Ok((String::new(), args));
    }
    if args.len() > 1 && !args[1].starts_with('-') {
        bail!("usage: burner deploy <service-name> -c \"<command>\" -l \"<location>\"");
    }
    Ok((args[0].clone(), &args[1..]))
}

fn default_deploy_folder() -> Result<String> {
    let path = std::path::Path::new("burner-deploy");
    let info = std::fs::metadata(path).map_err(|err| {
        if err.kind() == std::io::ErrorKind::NotFound {
            anyhow!("remote deploy without -l requires a ./burner-deploy directory")
        } else {
            anyhow!("read burner-deploy: {err}")
        }
    })?;
    if !info.is_dir() {
        bail!("./burner-deploy exists but is not a directory");
    }
    normalize_location("burner-deploy")
}

fn usage_error(prefix: &str) -> String {
    let lines = [
        "burner online -p <port>",
        "burner link -url \"http://server\" -p 9771 -k \"<api-key>\"",
        "burner deploy <service-name> -c \"<command>\" -l \"<location>\"",
        "burner run -c \"<command>\" -l \"<location>\" [--name <service-name>]",
        "burner list [-s <server-id>]",
        "burner logs <service-name> -n <line-count> [-s <server-id>]",
        "burner start <service-name> [-s <server-id>]",
        "burner stop <service-name> [-s <server-id>]",
        "burner restart <service-name> [-s <server-id>]",
    ];
    if prefix.is_empty() {
        lines.join("\n")
    } else {
        format!("{}\n{}", prefix, lines.join("\n"))
    }
}

fn value_or_dash(value: &str) -> String {
    if value.is_empty() {
        "-".into()
    } else {
        value.to_string()
    }
}

fn online_service_name(port: u16) -> String {
    format!("burner-online-{port}")
}

fn random_id() -> String {
    let mut buf = [0u8; 4];
    rand::thread_rng().fill_bytes(&mut buf);
    hex::encode(buf)
}

struct FlagValue<'a> {
    name: &'static str,
    target: &'a mut String,
}

impl<'a> FlagValue<'a> {
    fn new(name: &'static str, target: &'a mut String) -> Self {
        Self { name, target }
    }
}

struct BoolFlag<'a> {
    name: &'static str,
    target: &'a mut bool,
}

impl<'a> BoolFlag<'a> {
    fn new(name: &'static str, target: &'a mut bool) -> Self {
        Self { name, target }
    }
}

fn parse_flags<'a>(
    args: &'a [String],
    values: &mut [FlagValue<'_>],
    bools: &mut [BoolFlag<'_>],
) -> Result<Vec<String>> {
    let mut rest = Vec::new();
    let mut index = 0;
    while index < args.len() {
        let arg = &args[index];
        if let Some(flag) = bools.iter_mut().find(|flag| flag.name == arg) {
            *flag.target = true;
            index += 1;
            continue;
        }
        if let Some(flag) = values.iter_mut().find(|flag| flag.name == arg) {
            let value = args
                .get(index + 1)
                .ok_or_else(|| anyhow!("missing value for {}", flag.name))?;
            *flag.target = value.clone();
            index += 2;
            continue;
        }
        if arg.starts_with('-') {
            bail!("flag provided but not defined: {arg}");
        }
        rest.push(arg.clone());
        index += 1;
    }
    Ok(rest)
}

#[cfg(test)]
mod tests {
    use super::split_leading_name;

    #[test]
    fn split_leading_name_with_name() {
        let args = vec!["test".to_string(), "-c".to_string(), "bun run start".to_string()];
        let (name, rest) = split_leading_name(&args).unwrap();
        assert_eq!(name, "test");
        assert_eq!(rest, &args[1..]);
    }

    #[test]
    fn split_leading_name_flags_first() {
        let args = vec!["-c".to_string(), "bun run start".to_string(), "test".to_string()];
        let (name, rest) = split_leading_name(&args).unwrap();
        assert!(name.is_empty());
        assert_eq!(rest, &args[..]);
    }
}
