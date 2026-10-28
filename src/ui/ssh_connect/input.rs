//! Typed SSH connection syntax, independent of alias lookup, credentials and dialing.
//! Keep its legacy SSH quoting distinct from the settings argv-field grammar.

use crate::daemon::protocol::ssh_option_takes_value;
use crate::ui::i18n::{L10nKey, t, t_fmt};

fn parse_ssh_option_words(input: &str) -> Result<Vec<String>, ()> {
    let mut words = Vec::new();
    let mut current = String::new();
    let mut quote = None;
    let mut chars = input.chars();
    while let Some(ch) = chars.next() {
        match (quote, ch) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), '\\') => {
                if let Some(next) = chars.next() {
                    current.push(next);
                }
            }
            (Some(_), c) => current.push(c),
            (None, '\'' | '"') => quote = Some(ch),
            (None, c) if c.is_whitespace() => {
                if !current.is_empty() {
                    words.push(std::mem::take(&mut current));
                }
            }
            (None, c) => current.push(c),
        }
    }
    if quote.is_some() {
        return Err(());
    }
    if !current.is_empty() {
        words.push(current);
    }
    Ok(words)
}

/// Syntax-only transient connection data; connection consumers resolve aliases and credentials.
pub(crate) struct ParsedSshConnect {
    pub profile: crate::core::ssh_profile::SshProfile,
    pub proxy_jump: Option<String>,
}

/// Parse the existing typed SSH forms and return the existing localized validation errors.
pub(crate) fn parse_ssh_connect_input(input: &str) -> Result<ParsedSshConnect, String> {
    use crate::core::ssh_profile::{SshProfile, parse_quick_connect};

    let mut words = parse_ssh_option_words(input)
        .map_err(|_| t(L10nKey::AppSshParseUnbalancedQuotes).to_string())?;
    if words.first().is_some_and(|word| word == "ssh") {
        words.remove(0);
    }

    let mut target: Option<String> = None;
    let mut user: Option<String> = None;
    let mut port: Option<u16> = None;
    let mut identities: Vec<String> = Vec::new();
    let mut jump: Option<String> = None;

    let mut i = 0;
    while i < words.len() {
        let word = words[i].clone();
        if word == "--" {
            return Err(t(L10nKey::AppSshParseNoRemoteCommands).to_string());
        }
        if let Some((flag, attached)) = ssh_short_flag(&word) {
            let value = if ssh_option_takes_value(flag) {
                if !attached.is_empty() {
                    attached
                } else {
                    i += 1;
                    match words.get(i) {
                        Some(v) => v.clone(),
                        None => {
                            return Err(t_fmt(
                                L10nKey::AppSshParseFlagNeedsValue,
                                &[("flag", &flag.to_string())],
                            ));
                        }
                    }
                }
            } else {
                String::new()
            };
            match flag {
                'p' => {
                    port = Some(
                        value
                            .parse::<u16>()
                            .ok()
                            .filter(|&p| p != 0)
                            .ok_or_else(|| {
                                t_fmt(L10nKey::AppSshParseInvalidPort, &[("value", &value)])
                            })?,
                    )
                }
                'l' => user = Some(value),
                'i' => identities.push(value),
                'J' => jump = Some(value),
                'o' => apply_ssh_o_option(&value, &mut user, &mut port, &mut jump)?,
                _ => {}
            }
        } else if word.starts_with('-') {
            return Err(t_fmt(
                L10nKey::AppSshParseUnsupportedOption,
                &[("option", &word)],
            ));
        } else if target.is_none() {
            target = Some(word);
        } else {
            return Err("Remote commands aren't supported here".to_string());
        }
        i += 1;
    }

    let target = target.ok_or_else(|| t(L10nKey::AppSshParseEnterHost).to_string())?;
    let qc = parse_quick_connect(&target)
        .ok_or_else(|| t_fmt(L10nKey::AppSshParseBadHost, &[("host", &target)]))?;

    let mut profile = SshProfile::new(qc.host.clone());
    profile.host = qc.host;
    profile.port = port.or(qc.port).unwrap_or(22);
    if let Some(user) = user.or(qc.user) {
        profile.user = user;
    }
    profile.identity_files = identities;

    Ok(ParsedSshConnect {
        profile,
        proxy_jump: jump,
    })
}

fn ssh_short_flag(word: &str) -> Option<(char, String)> {
    let rest = word.strip_prefix('-')?;
    if rest.is_empty() || rest.starts_with('-') {
        return None;
    }
    let mut chars = rest.chars();
    let flag = chars.next()?;
    Some((flag, chars.as_str().to_string()))
}

fn apply_ssh_o_option(
    value: &str,
    user: &mut Option<String>,
    port: &mut Option<u16>,
    jump: &mut Option<String>,
) -> Result<(), String> {
    let Some((name, val)) = value.split_once('=') else {
        return Ok(());
    };
    match name.to_ascii_lowercase().as_str() {
        "user" => *user = Some(val.to_string()),
        "port" => {
            *port = Some(
                val.parse::<u16>()
                    .ok()
                    .filter(|&p| p != 0)
                    .ok_or_else(|| t_fmt(L10nKey::AppSshParseInvalidPort, &[("value", val)]))?,
            )
        }
        "proxyjump" => *jump = Some(val.to_string()),
        _ => {}
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{parse_ssh_connect_input, parse_ssh_option_words};

    #[test]
    fn parses_ssh_option_words_with_quotes() {
        assert_eq!(
            parse_ssh_option_words("-p 2222 -J 'jump host' -o \"User=dev\"").unwrap(),
            vec!["-p", "2222", "-J", "jump host", "-o", "User=dev"]
        );
    }

    #[test]
    fn rejects_unclosed_ssh_option_quote() {
        assert!(parse_ssh_option_words("-J 'jump").is_err());
    }

    #[test]
    fn parses_typed_connect_into_native_profile() {
        let p = parse_ssh_connect_input("ssh deploy@10.0.0.5:2222").unwrap();
        assert_eq!(p.profile.host, "10.0.0.5");
        assert_eq!(p.profile.user, "deploy");
        assert_eq!(p.profile.port, 2222);
        assert!(p.proxy_jump.is_none());
    }

    #[test]
    fn parses_typed_connect_flags_and_jump() {
        let p =
            parse_ssh_connect_input("ssh -p 2222 -l dev -i ~/.ssh/id_ed25519 -J 'jump host' host")
                .unwrap();
        assert_eq!(p.profile.host, "host");
        assert_eq!(p.profile.user, "dev");
        assert_eq!(p.profile.port, 2222);
        assert_eq!(
            p.profile.identity_files,
            vec!["~/.ssh/id_ed25519".to_string()]
        );
        assert_eq!(p.proxy_jump.as_deref(), Some("jump host"));

        let p = parse_ssh_connect_input("host -p2222 -o User=deploy -o Port=2200").unwrap();
        assert_eq!(p.profile.user, "deploy");
        assert_eq!(p.profile.port, 2200);
    }

    #[test]
    fn explicit_flags_override_target_userhost() {
        let p = parse_ssh_connect_input("ssh me@host:22 -l other -p 2200").unwrap();
        assert_eq!(p.profile.user, "other");
        assert_eq!(p.profile.port, 2200);
    }

    #[test]
    fn rejects_bad_typed_connect_lines() {
        assert!(parse_ssh_connect_input("ssh -p 2222").is_err());
        assert!(parse_ssh_connect_input("ssh dev uptime").is_err());
        assert!(parse_ssh_connect_input("ssh -- dev").is_err());
        assert!(parse_ssh_connect_input("ssh 'host").is_err());
        assert!(parse_ssh_connect_input("ssh host -p 0").is_err());
    }

    #[test]
    fn typed_identity_paths_keep_the_existing_ssh_escape_rules() {
        let parsed = parse_ssh_connect_input(
            r#"ssh -i C:\lyrel-fixtures\id -i "C:\\Program Files\\keys\\id" host"#,
        )
        .unwrap();
        assert_eq!(
            parsed.profile.identity_files,
            vec![r"C:\lyrel-fixtures\id", r"C:\Program Files\keys\id"]
        );
    }

    #[test]
    fn typed_connection_errors_keep_their_existing_categories() {
        use crate::ui::i18n::{L10nKey, t, t_fmt};

        for (input, expected) in [
            ("ssh 'host", t(L10nKey::AppSshParseUnbalancedQuotes).to_string()),
            ("ssh -p", t_fmt(L10nKey::AppSshParseFlagNeedsValue, &[("flag", "p")])),
            ("ssh host -p 0", t_fmt(L10nKey::AppSshParseInvalidPort, &[("value", "0")])),
            ("ssh -- host", t(L10nKey::AppSshParseNoRemoteCommands).to_string()),
            ("ssh host uptime", "Remote commands aren't supported here".to_string()),
        ] {
            assert_eq!(parse_ssh_connect_input(input).err(), Some(expected), "{input:?}");
        }
    }
}
