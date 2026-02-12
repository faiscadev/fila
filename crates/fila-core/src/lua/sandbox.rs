use mlua::{Lua, LuaOptions, StdLib};

/// Create a sandboxed Lua VM with only safe standard libraries.
///
/// Provides: math, string, table, utf8, and base functions (type, tostring, pcall, error, etc.)
/// Removes: io, os, package, debug, loadfile, dofile, load (filesystem/code loading)
pub fn create_sandbox() -> mlua::Result<Lua> {
    let safe_libs = StdLib::MATH | StdLib::STRING | StdLib::TABLE | StdLib::UTF8;
    let lua = Lua::new_with(safe_libs, LuaOptions::default())?;

    // loadfile/dofile/load are Lua core globals — always present regardless of
    // StdLib flags. Must nil them explicitly to complete the sandbox.
    let globals = lua.globals();
    for name in &["loadfile", "dofile", "load"] {
        globals.set(*name, mlua::Value::Nil)?;
    }

    Ok(lua)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sandbox_provides_safe_libs() {
        let lua = create_sandbox().unwrap();

        let math_result: f64 = lua.load("return math.sqrt(144)").eval().expect("math.sqrt");
        assert!((math_result - 12.0).abs() < f64::EPSILON);

        let string_result: String = lua
            .load(r#"return string.upper("hello")"#)
            .eval()
            .expect("string.upper");
        assert_eq!(string_result, "HELLO");

        let table_result: i64 = lua
            .load(
                r#"
                local t = {3, 1, 2}
                table.sort(t)
                return t[1]
            "#,
            )
            .eval()
            .expect("table.sort");
        assert_eq!(table_result, 1);
    }

    #[test]
    fn sandbox_blocks_dangerous_functions() {
        let lua = create_sandbox().unwrap();

        let dangerous_accesses = [
            ("io", "io.open('/etc/passwd', 'r')"),
            ("os", "os.execute('echo pwned')"),
            ("loadfile", "loadfile('init.lua')"),
            ("dofile", "dofile('init.lua')"),
            ("load", "load('return 1')()"),
            ("require", "require('os')"),
        ];

        for (name, code) in &dangerous_accesses {
            let result = lua.load(*code).exec();
            assert!(result.is_err(), "{name} should not be available in sandbox");
        }
    }
}
