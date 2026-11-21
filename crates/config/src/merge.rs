use codex_network_proxy::normalize_host;
use toml::Value as TomlValue;

/// Merge config `overlay` into `base`, giving `overlay` precedence.
pub fn merge_toml_values(base: &mut TomlValue, overlay: &TomlValue) {
    merge_toml_values_at_path(base, overlay.clone(), &mut Vec::new());
}

pub fn is_structured_feature_path<S: AsRef<str>>(path: &[S]) -> bool {
    let (features, feature) = match path {
        [profiles, _, features, feature] if profiles.as_ref() == "profiles" => (features, feature),
        [features, feature] => (features, feature),
        _ => return false,
    };

    features.as_ref() == "features" && codex_features::is_structured_feature(feature.as_ref())
}

fn merge_toml_values_at_path(base: &mut TomlValue, overlay: TomlValue, path: &mut Vec<String>) {
    if let TomlValue::Table(base_table) = &mut *base
        && let TomlValue::Table(mut overlay_table) = overlay
    {
        if is_network_domains_path(path) {
            normalize_network_domain_keys(base_table);
            normalize_network_domain_keys(&mut overlay_table);
        }
        if is_shell_environment_filters_path(path) {
            normalize_case_insensitive_keys(base_table);
            normalize_case_insensitive_keys(&mut overlay_table);
        }
        if path.split_last().is_some_and(|(field, feature_path)| {
            field == "credentials" && is_structured_feature_path(feature_path)
        }) {
            for (provider_id, provider) in &overlay_table {
                let Some(overlay_sources) = provider.get("env").and_then(TomlValue::as_array)
                else {
                    continue;
                };
                base_table.retain(|existing_id, existing| {
                    existing_id == provider_id
                        || !overlay_table
                            .get(existing_id)
                            .and_then(|provider| provider.get("env"))
                            .or_else(|| existing.get("env"))
                            .and_then(TomlValue::as_array)
                            .is_some_and(|existing_sources| {
                                existing_sources.iter().filter_map(TomlValue::as_str).any(
                                    |existing_source| {
                                        overlay_sources.iter().filter_map(TomlValue::as_str).any(
                                            |overlay_source| {
                                                existing_source == overlay_source
                                                    || cfg!(windows)
                                                        && existing_source
                                                            .eq_ignore_ascii_case(overlay_source)
                                            },
                                        )
                                    },
                                )
                            })
                });
            }
        }
        for (key, value) in overlay_table {
            path.push(key.clone());
            if let Some(existing) = base_table.get_mut(&key) {
                merge_toml_values_at_path(existing, value, path);
            } else {
                base_table.insert(key, value);
            }
            path.pop();
        }
    } else {
        *base = overlay;
    }
}

fn is_shell_environment_filters_path(path: &[String]) -> bool {
    matches!(
        path,
        [policy, filters]
            if policy == "shell_environment_policy" && filters == "filters"
    )
}

/// Looks up a shell-environment filter pattern while ignoring case.
pub fn shell_environment_filter_entry<'a>(
    root: &'a TomlValue,
    path: &[String],
) -> Option<(&'a String, &'a TomlValue)> {
    let [policy, filters, pattern] = path else {
        return None;
    };
    if policy != "shell_environment_policy" || filters != "filters" {
        return None;
    }

    let pattern = pattern.to_lowercase();
    root.get(policy)?
        .get(filters)?
        .as_table()?
        .iter()
        .find(|(candidate, _)| candidate.to_lowercase() == pattern)
}

fn is_network_domains_path(path: &[String]) -> bool {
    matches!(
        path,
        [permissions, _, network, domains]
            if permissions == "permissions" && network == "network" && domains == "domains"
    ) || matches!(
        path,
        [application, network, domains]
            if application == "application" && network == "network" && domains == "domains"
    )
}

fn normalize_network_domain_keys(table: &mut toml::map::Map<String, TomlValue>) {
    let entries = std::mem::take(table);
    for (pattern, value) in entries {
        table.insert(normalize_host(&pattern), value);
    }
}

fn normalize_case_insensitive_keys(table: &mut toml::map::Map<String, TomlValue>) {
    let entries = std::mem::take(table);
    for (key, value) in entries {
        table.insert(key.to_lowercase(), value);
    }
}

#[cfg(test)]
#[path = "merge_tests.rs"]
mod tests;
