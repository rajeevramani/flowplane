use regex::Regex;
use std::fmt::Write as _;
use uuid::Uuid;

/// Generates unique, readable names and paths per test run.
/// Pass a stable `base` (e.g., the test name) to ensure determinism within a test.
#[derive(Clone, Debug)]
pub struct UniqueNamer {
    base: String,
    suffix: String,
}

impl UniqueNamer {
    /// Create a new namer for the given test base. The suffix is a short, random identifier
    /// to ensure uniqueness across concurrent tests while remaining stable within this test.
    pub fn for_test(base: &str) -> Self {
        let short = Uuid::new_v4().to_string();
        // Keep it short but collision-resistant for practical parallelism
        let suffix = short.chars().take(8).collect::<String>();
        Self { base: base.to_string(), suffix }
    }

    /// Returns a unique test id derived from base + suffix.
    pub fn test_id(&self) -> String {
        format!("{}-{}", sanitize(&self.base), self.suffix)
    }

    /// Returns a unique, local-only domain for vhost testing.
    /// Example: `smoke-1a2b3c4d.e2e.local`
    pub fn domain(&self) -> String {
        format!("{}.e2e.local", self.test_id())
    }

    /// Returns a unique base path prefix for routes in this test.
    /// Example: `/e2e/smoke-1a2b3c4d`
    pub fn base_path(&self) -> String {
        format!("/e2e/{}", self.test_id())
    }

    /// Returns a path under the unique base path.
    /// Example: `/e2e/smoke-1a2b3c4d/hello`
    pub fn path(&self, route: &str) -> String {
        let mut buf = String::with_capacity(self.base_path().len() + 1 + route.len());
        let _ = write!(&mut buf, "{}/{}", self.base_path(), trim_leading_slash(route));
        buf
    }
}

fn trim_leading_slash(s: &str) -> &str {
    s.strip_prefix('/').unwrap_or(s)
}

fn sanitize(input: &str) -> String {
    // Lowercase, keep alnum and dashes only; replace invalid with dash
    let re = Regex::new(r"[^a-z0-9-]+").unwrap();
    let lower = input.to_ascii_lowercase().replace('_', "-");
    let cleaned = re.replace_all(&lower, "-");
    // collapse multiple dashes
    let re2 = Regex::new(r"-+").unwrap();
    let collapsed = re2.replace_all(&cleaned, "-");
    collapsed.trim_matches('-').to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn naming_shapes_are_reasonable() {
        let n = UniqueNamer::for_test("Smoke_Boot_And_Route");
        assert!(n.domain().ends_with(".e2e.local"));
        assert!(n.base_path().starts_with("/e2e/"));
        assert!(n.path("hello").contains(&n.base_path()));
    }
}
