//! Port of the original `helpers.generateTasks` / `generateFile`: emit a
//! locustfile for anyone who still wants to drive the test with Locust.

pub fn generate_locustfile(paths: &[String]) -> String {
    let mut tasks = String::new();
    // The JS version started at index 1 and silently skipped the first link.
    for (i, path) in paths.iter().enumerate() {
        let path = path.replace('\\', "\\\\").replace('"', "\\\"");
        tasks.push_str(&format!(
            "    @task\n    def f{i}(self):\n        self.client.get(\"{path}\")\n\n"
        ));
    }
    if tasks.is_empty() {
        tasks.push_str("    pass\n");
    }
    format!(
        "from locust import HttpUser, task, between\n\n\n\
         class QuickstartUser(HttpUser):\n    wait_time = between(5, 9)\n\n{tasks}"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generates_one_task_per_path() {
        let out = generate_locustfile(&["/".into(), "/a\"b".into()]);
        assert!(out.contains("from locust import"));
        assert!(out.contains("def f0(self):\n        self.client.get(\"/\")"));
        assert!(out.contains("self.client.get(\"/a\\\"b\")"));
    }

    #[test]
    fn empty_is_still_valid_python() {
        assert!(generate_locustfile(&[]).contains("    pass\n"));
    }
}
