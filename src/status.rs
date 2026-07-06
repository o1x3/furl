//! Process exit statuses.

/// Exit codes reported by the `furl`/`furls` binaries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i32)]
pub enum ExitStatus {
    /// Normal completion, including HTTP error statuses when neither
    /// `--check-status` nor download mode is in effect.
    Success = 0,
    /// Generic errors: connection failures, content-range violations,
    /// incomplete downloads, and all usage errors.
    Error = 1,
    /// The request exceeded `--timeout`.
    ErrorTimeout = 2,
    /// Final response was 3xx without `--follow` (only when checking).
    ErrorHttp3xx = 3,
    /// Final response was 4xx (only when checking).
    ErrorHttp4xx = 4,
    /// Final response was 5xx (only when checking).
    ErrorHttp5xx = 5,
    /// The redirect count reached `--max-redirects` while following.
    ErrorTooManyRedirects = 6,
    /// Reserved for plugin failures.
    PluginError = 7,
    /// Interrupted with Ctrl-C (128 + SIGINT).
    ErrorCtrlC = 130,
}

impl ExitStatus {
    pub fn code(self) -> i32 {
        self as i32
    }

    /// Map the final response's HTTP status to an exit status.
    ///
    /// Only consulted when `--check-status` or download mode is active.
    /// A 3xx final response maps to success under `--follow` (having been
    /// followed, a remaining 3xx is one without a follow-up, e.g. 304).
    pub fn from_http_status(status: u16, follow: bool) -> ExitStatus {
        match status {
            300..=399 if !follow => ExitStatus::ErrorHttp3xx,
            400..=499 => ExitStatus::ErrorHttp4xx,
            500..=599 => ExitStatus::ErrorHttp5xx,
            _ => ExitStatus::Success,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn http_status_mapping() {
        assert_eq!(
            ExitStatus::from_http_status(301, false),
            ExitStatus::ErrorHttp3xx
        );
        assert_eq!(
            ExitStatus::from_http_status(301, true),
            ExitStatus::Success
        );
        assert_eq!(
            ExitStatus::from_http_status(304, true),
            ExitStatus::Success
        );
        assert_eq!(
            ExitStatus::from_http_status(404, false),
            ExitStatus::ErrorHttp4xx
        );
        assert_eq!(
            ExitStatus::from_http_status(404, true),
            ExitStatus::ErrorHttp4xx
        );
        assert_eq!(
            ExitStatus::from_http_status(500, true),
            ExitStatus::ErrorHttp5xx
        );
        assert_eq!(
            ExitStatus::from_http_status(200, false),
            ExitStatus::Success
        );
        assert_eq!(
            ExitStatus::from_http_status(101, false),
            ExitStatus::Success
        );
    }

    #[test]
    fn codes() {
        assert_eq!(ExitStatus::Success.code(), 0);
        assert_eq!(ExitStatus::ErrorCtrlC.code(), 130);
    }
}
