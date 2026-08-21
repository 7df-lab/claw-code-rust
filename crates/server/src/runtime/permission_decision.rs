use devo_protocol::native::item::ApprovalDecisionSource;

/// One authorization-layer decision before it is converted into an execution
/// grant or an interactive control request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum AuthorizationDecision {
    Allow {
        source: ApprovalDecisionSource,
    },
    Ask {
        source: ApprovalDecisionSource,
    },
    Deny {
        source: ApprovalDecisionSource,
        reason: String,
    },
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;

    #[test]
    fn every_authorization_outcome_has_an_explicit_source() {
        let decisions = [
            AuthorizationDecision::Allow {
                source: ApprovalDecisionSource::StaticPolicy,
            },
            AuthorizationDecision::Ask {
                source: ApprovalDecisionSource::ExecPolicy,
            },
            AuthorizationDecision::Deny {
                source: ApprovalDecisionSource::Hook,
                reason: "blocked".to_string(),
            },
        ];

        assert_eq!(
            decisions.map(|decision| match decision {
                AuthorizationDecision::Allow { source }
                | AuthorizationDecision::Ask { source }
                | AuthorizationDecision::Deny { source, .. } => source,
            }),
            [
                ApprovalDecisionSource::StaticPolicy,
                ApprovalDecisionSource::ExecPolicy,
                ApprovalDecisionSource::Hook,
            ]
        );
    }
}
