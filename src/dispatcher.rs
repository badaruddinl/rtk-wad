//! Dispatcher contracts shared by discovery, route selection, and execution.
//!
//! User arguments deliberately remain `OsString`s.  A dispatcher never joins
//! them into a shell command: shell syntax (`cd`, pipes, redirects, aliases)
//! belongs to the invoking shell and is not part of this contract.

use std::ffi::OsString;
use std::path::PathBuf;

use crate::providers::model::BinaryIdentity;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CommandSpec {
    pub(crate) executable: OsString,
    pub(crate) arguments: Vec<OsString>,
    pub(crate) cwd: Option<PathBuf>,
    pub(crate) environment: Vec<(OsString, OsString)>,
    pub(crate) environment_policy: EnvironmentPolicy,
    pub(crate) interactive: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum EnvironmentPolicy {
    Inherit,
    Isolated,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum RouteCandidate {
    Windows {
        executable: OsString,
        cwd: Option<PathBuf>,
    },
    Wsl1 {
        distro: String,
        executable: OsString,
        cwd: PathBuf,
    },
    Wsl2 {
        distro: String,
        executable: OsString,
        cwd: PathBuf,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum OutputAdapter {
    Raw,
    Rtk { executable: OsString },
}

impl OutputAdapter {
    pub(crate) fn as_str(&self) -> &'static str {
        match self {
            Self::Raw => "raw",
            Self::Rtk { .. } => "rtk",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct DecisionReason(pub(crate) String);

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ExecutionPlan {
    pub(crate) request: CommandSpec,
    pub(crate) candidate: RouteCandidate,
    pub(crate) adapter: OutputAdapter,
    pub(crate) expected_identity: Option<BinaryIdentity>,
    pub(crate) explanation: Vec<DecisionReason>,
}

#[allow(dead_code)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ProvisioningPlan {
    pub(crate) summary: String,
}

/// Provisioning is intentionally separate from resolution and execution.
/// Implementations may only apply a plan after a caller has obtained explicit
/// user approval.
#[allow(dead_code)]
pub(crate) trait Provisioner {
    fn plan(&self, request: &CommandSpec) -> ProvisioningPlan;
    fn apply(&self, plan: &ProvisioningPlan, user_approved: bool) -> Result<(), String>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raw_plan_preserves_literal_arguments_without_a_shell() {
        let request = CommandSpec {
            executable: OsString::from("go"),
            arguments: vec![OsString::from("version"), OsString::from("$literal & text")],
            cwd: None,
            environment: Vec::new(),
            environment_policy: EnvironmentPolicy::Isolated,
            interactive: false,
        };
        let plan = ExecutionPlan {
            request: request.clone(),
            candidate: RouteCandidate::Wsl2 {
                distro: "Ubuntu".to_owned(),
                executable: OsString::from("/usr/local/go/bin/go"),
                cwd: PathBuf::from("/mnt/c/work"),
            },
            adapter: OutputAdapter::Raw,
            expected_identity: None,
            explanation: vec![DecisionReason(
                "RTK output adaptation is disabled".to_owned(),
            )],
        };
        assert_eq!(plan.request, request);
        assert_eq!(plan.adapter.as_str(), "raw");
    }
}
