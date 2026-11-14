//! Production boot configuration; independent of guest-test selection.

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Root {
    Ram,
    Disk,
}

impl Root {
    pub const fn name(self) -> &'static str {
        match self {
            Self::Ram => "ram",
            Self::Disk => "disk",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Config<'a> {
    pub root: Option<Root>,
    pub init: &'a str,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(i32)]
pub enum Error {
    CommandLine = -32,
    Root = -33,
    Init = -34,
    InitWithoutRoot = -35,
    Duplicate = -36,
    MixedTest = -37,
}

impl<'a> Config<'a> {
    pub fn parse(command_line: &'a str) -> Result<Self, Error> {
        if command_line.len() > 256 || !command_line.is_ascii() {
            return Err(Error::CommandLine);
        }
        let mut root = None;
        let mut init = None;
        for option in command_line.split_ascii_whitespace() {
            let (key, value) = option.split_once('=').ok_or(Error::CommandLine)?;
            match key {
                "root" => {
                    if root.is_some() {
                        return Err(Error::Duplicate);
                    }
                    root = Some(match value {
                        "ram" => Root::Ram,
                        "disk" => Root::Disk,
                        _ => return Err(Error::Root),
                    });
                }
                "init" => {
                    if init.is_some() {
                        return Err(Error::Duplicate);
                    }
                    if value.len() > 128
                        || !value.starts_with('/')
                        || value[1..].split('/').any(|part| {
                            part.is_empty() || part == "." || part == ".." || part.contains('\0')
                        })
                    {
                        return Err(Error::Init);
                    }
                    init = Some(value);
                }
                _ => return Err(Error::CommandLine),
            }
        }
        if root.is_none() && init.is_some() {
            return Err(Error::InitWithoutRoot);
        }
        Ok(Self {
            root,
            init: init.unwrap_or("/bin/init"),
        })
    }

    pub fn validate_test(self, selected: bool) -> Result<(), Error> {
        if selected && self.root.is_some() {
            return Err(Error::MixedTest);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_to_networking_and_selects_root_and_init_independently() {
        let default = Config::parse("").unwrap();
        assert_eq!(default.root, None);
        for (value, root) in [("ram", Root::Ram), ("disk", Root::Disk)] {
            let line = std::format!("root={value}");
            let config = Config::parse(&line).unwrap();
            assert_eq!(config.root, Some(root));
            assert_eq!(config.init, "/bin/init");
        }
        assert_eq!(
            Config::parse("init=/bin/other root=disk").unwrap().init,
            "/bin/other"
        );
    }

    #[test]
    fn malformed_duplicate_and_mixed_configuration_is_not_a_fallback() {
        for line in [
            "root=other",
            "root=",
            "init=/bin/init",
            "mode=run",
            "root=ram root=disk",
            "root=ram init=/x init=/y",
            "root=ram init=",
            "root=ram init=relative",
            "root=ram init=/bin/../init",
            "root=ram init=/bin//init",
            "root=ram init=/",
        ] {
            assert!(Config::parse(line).is_err(), "{line}");
        }
        assert_eq!(
            Config::parse("root=ram").unwrap().validate_test(true),
            Err(Error::MixedTest)
        );
        assert_eq!(Config::parse("").unwrap().validate_test(true), Ok(()));
    }
}
