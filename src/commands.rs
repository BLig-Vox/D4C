use serde_json::Value;

#[derive(Clone, Debug)]
pub struct Command {
    pub token: String,
    pub description: String,
    pub source: String,
    pub aliases: Vec<String>,
    pub args: Vec<Value>,
}
#[derive(Clone, Debug, PartialEq)]
pub struct Suggestion {
    pub label: String,
    pub description: String,
    pub replacement: String,
    pub skill: bool,
}
pub fn parse(payload: &Value) -> Vec<Command> {
    let mut commands: Vec<_> = payload["commands"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|row| {
            let name = row["name"].as_str()?.trim_start_matches('/');
            if name.is_empty() {
                return None;
            }
            Some(Command {
                token: format!("/{name}"),
                description: row["description"].as_str().unwrap_or("").into(),
                source: row["source"].as_str().unwrap_or("native").into(),
                aliases: row["textAliases"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|a| a.as_str().map(str::to_owned))
                    .collect(),
                args: row["args"].as_array().cloned().unwrap_or_default(),
            })
        })
        .collect();
    commands.retain(|c| !matches!(c.token.as_str(), "/permission" | "/permissions"));
    commands.push(Command {
        token: "/permissions".into(),
        description: "Change this chat’s permissions".into(),
        source: "native".into(),
        aliases: vec!["/permission".into()],
        args: vec![serde_json::json!({"name":"mode","choices":[
            {"value":"default","label":"Workspace writes · ask for approval"},
            {"value":"read-only","label":"Read only · ask for approval"},
            {"value":"yolo","label":"Full filesystem and network · no approval prompts"}
        ]})],
    });
    commands.sort_by(|a, b| a.token.cmp(&b.token));
    commands.dedup_by(|a, b| a.token == b.token);
    commands
}
pub fn suggestions(commands: &[Command], input: &str) -> Vec<Suggestion> {
    if !input.starts_with('/') || input.contains('\n') {
        return vec![];
    }
    if !input.contains(char::is_whitespace) {
        let query = input.to_lowercase();
        return commands
            .iter()
            .filter(|c| {
                c.token.to_lowercase().starts_with(&query)
                    || c.aliases
                        .iter()
                        .any(|a| a.to_lowercase().starts_with(&query))
            })
            .map(|c| Suggestion {
                label: c.token.clone(),
                description: c.description.clone(),
                replacement: format!("{} ", c.token),
                skill: c.source == "skill",
            })
            .take(40)
            .collect();
    }
    let (token, rest) = input.split_once(char::is_whitespace).unwrap();
    if token == "/skill" && !rest.trim_start().contains(char::is_whitespace) {
        let query = rest.trim_start().to_lowercase();
        return commands
            .iter()
            .filter(|c| {
                c.source == "skill"
                    && c.token
                        .trim_start_matches('/')
                        .to_lowercase()
                        .starts_with(&query)
            })
            .map(|c| Suggestion {
                label: c.token.clone(),
                description: c.description.clone(),
                replacement: format!("{} ", c.token),
                skill: true,
            })
            .take(40)
            .collect();
    }
    let Some(command) = commands
        .iter()
        .find(|c| c.token == token || c.aliases.iter().any(|a| a == token))
    else {
        return vec![];
    };
    let completed = rest.split_whitespace().count().saturating_sub(usize::from(
        !rest.ends_with(char::is_whitespace) && !rest.is_empty(),
    ));
    let Some(arg) = command.args.get(completed) else {
        return vec![];
    };
    let query = if rest.ends_with(char::is_whitespace) {
        ""
    } else {
        rest.split_whitespace().last().unwrap_or("")
    };
    let prefix = if query.is_empty() {
        input
    } else {
        &input[..input.len() - query.len()]
    };
    arg["choices"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|choice| {
            let value = choice.as_str().or_else(|| choice["value"].as_str())?;
            if !value.to_lowercase().starts_with(&query.to_lowercase()) {
                return None;
            }
            Some(Suggestion {
                label: value.into(),
                description: choice["label"]
                    .as_str()
                    .or_else(|| arg["description"].as_str())
                    .unwrap_or("")
                    .into(),
                replacement: format!("{prefix}{value} "),
                skill: false,
            })
        })
        .take(40)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn catalog() -> Vec<Command> {
        parse(&json!({"commands":[
            {"name":"model","description":"Choose a model","textAliases":["/model","/m"],"args":[{"name":"model","choices":["terra","sol"]}]},
            {"name":"github","source":"skill","description":"Work with GitHub"},
            {"name":"think","args":[{"name":"level","choices":[{"value":"medium","label":"Balanced"},"high"]}]}
        ]}))
    }
    #[test]
    fn discovers_skills_and_aliases() {
        assert_eq!(suggestions(&catalog(), "/m")[0].replacement, "/model ");
        assert!(suggestions(&catalog(), "/git")[0].skill);
        assert_eq!(
            suggestions(&catalog(), "/skill git")[0].replacement,
            "/github "
        );
    }
    #[test]
    fn completes_argument_choices_without_executing() {
        assert_eq!(
            suggestions(&catalog(), "/think m")[0].replacement,
            "/think medium "
        );
        assert_eq!(suggestions(&catalog(), "/model ").len(), 2);
        assert!(suggestions(&catalog(), "/model terra ").is_empty());
        assert!(suggestions(&catalog(), "hello /git").is_empty());
        assert!(suggestions(&catalog(), "/model\ntext").is_empty());
    }
}
