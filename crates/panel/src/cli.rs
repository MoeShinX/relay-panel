//! v1.2.5: command-line recovery, run against the same database the server uses.
//!
//! Exists because the only way to recover a lost admin password was to edit the
//! database by hand — pasting a bcrypt placeholder string full of `$` through a
//! shell, which mangles under quoting, only worked for user id 1, and failed
//! SILENTLY when the pattern did not match. An operator locked out of their own
//! panel is exactly the person who should not be debugging shell escaping.
//!
//! These subcommands are reachable only from the CLI. Anyone who can run them
//! already has the database file, so they add no attack surface — but they must
//! never be wired to an HTTP route.

use crate::db::repo::{NewAuditEntry, Repository};
use crate::service::password::{generate_password, hash_password};
use std::sync::Arc;

pub const USAGE: &str = "\
RelayPanel — self-hosted TCP/UDP forwarding panel

USAGE:
    relay-panel                                 start the server (default)
    relay-panel reset-admin-password [USER]     reset a password, print a new one
    relay-panel --help                          show this

reset-admin-password:
    USER defaults to `admin`. A strong password is generated and printed once —
    it is not read from the arguments, because anything passed on the command
    line lands in shell history and is visible to `ps` on a shared host.

    All of that account's existing sessions are signed out.
";

/// Reset one account's password to a freshly generated one. Returns the process
/// exit code.
pub async fn reset_admin_password(db: Arc<dyn Repository>, username: &str) -> i32 {
    let user = match db.find_by_username(username).await {
        Ok(Some(u)) => u,
        Ok(None) => {
            eprintln!("错误：用户 `{username}` 不存在。");
            eprintln!("提示：管理员账号默认叫 `admin`；也可以传入用户名，例如：");
            eprintln!("      relay-panel reset-admin-password someone");
            return 1;
        }
        Err(e) => {
            eprintln!("错误：查询用户失败：{e}");
            return 1;
        }
    };

    let password = generate_password();
    let hash = match hash_password(&password) {
        Ok(h) => h,
        Err(e) => {
            eprintln!("错误：密码哈希失败：{e}");
            return 1;
        }
    };

    // must_change_password = false: the generated password is strong enough to
    // keep, and someone who has just been locked out should not be made to pick
    // a new one before they can look at anything.
    //
    // The same call bumps token_version, which signs out every existing session
    // for this account. That matters most in the case where the reason for the
    // lockout is that somebody else got in.
    match db.admin_reset_password(user.id, &hash, false).await {
        Ok(0) => {
            eprintln!("错误：用户 `{username}` 在更新时已不存在。");
            return 1;
        }
        Ok(_) => {}
        Err(e) => {
            eprintln!("错误：更新密码失败：{e}");
            return 1;
        }
    }

    // Recorded so the operator can later see that a CLI reset happened and when.
    // actor_id is None — there is no authenticated user behind a shell command,
    // and inventing one would put a false name in the trail.
    let entry = NewAuditEntry {
        ts: chrono::Utc::now().format("%Y-%m-%d %H:%M:%S").to_string(),
        actor_id: None,
        actor_name: "命令行".to_string(),
        action: "reset_password".to_string(),
        target_type: "user".to_string(),
        target_id: user.id.to_string(),
        detail: format!("{username} — 通过命令行重置"),
    };
    if let Err(e) = db.record_audit(&entry).await {
        // Best-effort, exactly like the HTTP path: the password IS reset, and
        // failing the command now would leave the operator thinking it was not.
        eprintln!("警告：审计记录写入失败（密码已重置）：{e}");
    }

    println!();
    println!("  用户名： {username}");
    println!("  新密码： {password}");
    println!();
    println!("  该账号的所有登录会话已失效，请用上面的密码重新登录。");
    println!("  这串密码现在留在你的终端记录里，建议登录后到「个人中心 → 修改密码」换掉。");
    println!();
    0
}
