//! Static-site buildpack: serve a published directory behind nginx.
//!
//! Parity with Coolify's `deploy_static_buildpack`: an optional user build
//! step produces the assets, then a generated `FROM <static_image>` Dockerfile
//! copies the publish directory into the nginx web root with an SPA-friendly
//! default config. The generated Dockerfile is written as plaintext (no
//! secrets) so it is auditable in the deployment log.

use rustify_docker::BuildCommand;

use super::BuildCtx;

/// Filename (inside the workdir) of the generated nginx Dockerfile.
pub const NGINX_DOCKERFILE: &str = ".rustify-nginx.Dockerfile";

/// Generate the nginx Dockerfile. `publish_directory` is the repo-relative
/// directory of built assets (e.g. `/dist`); it is copied into
/// `/usr/share/nginx/html`.
pub fn nginx_dockerfile(static_image: &str, publish_directory: &str) -> String {
    let publish = publish_directory.trim_start_matches('/');
    let publish = if publish.is_empty() { "." } else { publish };
    format!(
        "FROM {static_image}\n\
         WORKDIR /usr/share/nginx/html\n\
         COPY ./{publish} /usr/share/nginx/html\n\
         RUN printf 'server {{\\n  listen 80;\\n  location / {{\\n    root /usr/share/nginx/html;\\n    try_files $uri $uri/ /index.html;\\n  }}\\n}}\\n' > /etc/nginx/conf.d/default.conf\n"
    )
}

/// `docker build` for the generated nginx Dockerfile.
pub fn docker_build(ctx: &BuildCtx) -> BuildCommand {
    BuildCommand {
        context: ctx.workdir.clone(),
        dockerfile: Some(format!("{}/{}", ctx.workdir, NGINX_DOCKERFILE)),
        image: ctx.image.clone(),
        build_args: vec![],
        no_cache: ctx.no_cache,
        pull: false,
        target: None,
        buildkit: true,
        env_file: Some(ctx.build_env_path.clone()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dockerfile_has_base_image_and_publish_copy() {
        let df = nginx_dockerfile("nginx:alpine", "/dist");
        assert!(df.contains("FROM nginx:alpine"));
        assert!(df.contains("COPY ./dist /usr/share/nginx/html"));
        assert!(df.contains("try_files"));
    }

    #[test]
    fn empty_publish_dir_copies_root() {
        let df = nginx_dockerfile("nginx:1.27", "/");
        assert!(df.contains("COPY ./. /usr/share/nginx/html"));
    }

    #[test]
    fn build_targets_generated_dockerfile() {
        let ctx = BuildCtx {
            deployment_uuid: "d".into(),
            workdir: "/artifacts/d".into(),
            image: "app:sha".into(),
            build_env_path: "/artifacts/build-time.env".into(),
            no_cache: false,
        };
        assert!(
            docker_build(&ctx)
                .render()
                .contains(&format!("-f /artifacts/d/{NGINX_DOCKERFILE}"))
        );
    }
}
