use thiserror::Error;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn execution_error_display() {
        let err = ToolExecutionError::PermissionDenied {
            reason: "not allowed".into(),
        };
        assert_eq!(err.to_string(), "permission denied: not allowed");

        let err = ToolExecutionError::ExecutionFailed {
            message: "oops".into(),
        };
        assert_eq!(err.to_string(), "execution failed: oops");

        let err = ToolExecutionError::Timeout {
            message: "took too long".into(),
        };
        assert_eq!(err.to_string(), "timeout: took too long");

        let err = ToolExecutionError::Interrupted;
        assert_eq!(err.to_string(), "interrupted");

        let err = ToolExecutionError::Internal {
            message: "bug".into(),
        };
        assert_eq!(err.to_string(), "internal: bug");
    }

    #[test]
    fn dispatch_error_display() {
        let err = ToolDispatchError::UnknownTool { name: "foo".into() };
        assert_eq!(err.to_string(), "unknown tool: foo");

        let err = ToolDispatchError::ExecutionError(ToolExecutionError::Timeout {
            message: "took too long".into(),
        });
        assert!(err.to_string().contains("timeout"));
    }

    #[test]
    fn dispatch_error_from_execution() {
        let exec = ToolExecutionError::ExecutionFailed {
            message: "fail".into(),
        };
        let dispatch: ToolDispatchError = exec.into();
        assert!(matches!(dispatch, ToolDispatchError::ExecutionError(_)));
    }
}

#[derive(Debug, Clone, Error)]
pub enum ToolExecutionError {
    #[error("permission denied: {reason}")]
    PermissionDenied { reason: String },

    #[error("execution failed: {message}")]
    ExecutionFailed { message: String },

    #[error("timeout: {message}")]
    Timeout { message: String },

    #[error("interrupted")]
    Interrupted,

    #[error("internal: {message}")]
    Internal { message: String },
}

#[derive(Debug, Clone, Error)]
pub enum ToolDispatchError {
    #[error("unknown tool: {name}")]
    UnknownTool { name: String },

    #[error("{0}")]
    ExecutionError(#[from] ToolExecutionError),
}
