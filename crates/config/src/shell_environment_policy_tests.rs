use super::*;
use pretty_assertions::assert_eq;

#[test]
fn shell_environment_policy_accepts_keyed_filters() {
    let filtered: ShellEnvironmentPolicyToml = toml::from_str(
        r#"
[filters]
"FLIP_TO_EXCLUDE" = "exclude"
"FLIP_TO_INCLUDE" = "include"
"#,
    )
    .expect("filters should be valid in config.toml");
    assert_eq!(
        filtered,
        ShellEnvironmentPolicyToml {
            filters: Some(BTreeMap::from([
                (
                    "FLIP_TO_EXCLUDE".to_string(),
                    ShellEnvironmentPolicyFilter::Exclude,
                ),
                (
                    "FLIP_TO_INCLUDE".to_string(),
                    ShellEnvironmentPolicyFilter::Include,
                ),
            ])),
            ..Default::default()
        }
    );
    let policy = ShellEnvironmentPolicy::from(filtered);
    assert_eq!(
        policy.exclude,
        vec![EnvironmentVariablePattern::new_case_insensitive(
            "FLIP_TO_EXCLUDE"
        )]
    );
    assert_eq!(
        policy.include_only,
        vec![EnvironmentVariablePattern::new_case_insensitive(
            "FLIP_TO_INCLUDE"
        )]
    );
}

#[test]
fn shell_environment_policy_rejects_retired_arrays() {
    for field in ["exclude", "include_only"] {
        let error =
            toml::from_str::<ShellEnvironmentPolicyToml>(&format!("{field} = ['SECRET_*']"))
                .expect_err("retired shell restrictions must fail closed");
        assert!(
            error
                .to_string()
                .contains("shell_environment_policy.filters")
        );
    }
}

#[test]
fn shell_environment_policy_rejects_case_variant_filters_within_layer() {
    let error = toml::from_str::<ShellEnvironmentPolicyToml>(
        r#"
[filters]
"AWS_*" = "exclude"
"aws_*" = "include"
"#,
    )
    .expect_err("case-variant filters in one layer should be rejected");

    assert!(
        error
            .to_string()
            .contains("duplicate shell environment filter")
    );
}

#[test]
fn shell_environment_policy_rejects_unicode_case_variant_filters_within_layer() {
    let error = toml::from_str::<ShellEnvironmentPolicyToml>(
        r#"
[filters]
"СЕКРЕТ_*" = "exclude"
"секрет_*" = "include"
"#,
    )
    .expect_err("Unicode case-variant filters in one layer should be rejected");

    assert!(
        error
            .to_string()
            .contains("duplicate shell environment filter")
    );
}
