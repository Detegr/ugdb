use gdbmi::commands::MiCommand;
use gdbmi::output::{ResultClass, ResultRecord};
use gdbmi::ExecuteError;

use log::error;

pub struct Command {
    cmd: Box<dyn FnMut(::UpdateParameters) -> Result<(), ExecuteError>>,
}

impl Command {
    fn new(cmd: Box<dyn FnMut(::UpdateParameters) -> Result<(), ExecuteError>>) -> Command {
        Command { cmd: cmd }
    }
    fn from_mi_with_msg(cmd: MiCommand, success_msg: &'static str) -> Command {
        Command::new(Box::new(move |p: ::UpdateParameters| {
            let res = p.gdb.mi.execute(cmd.clone()).map(|_| ());
            if res.is_ok() {
                p.message_sink.send(success_msg);
            }
            res
        }))
    }
    fn from_mi(cmd: MiCommand) -> Command {
        Command::new(Box::new(move |p: ::UpdateParameters| {
            p.gdb.mi.execute(cmd.clone()).map(|_| ())
        }))
    }
}

pub enum CommandState {
    Idle,
    WaitingForConfirmation(Command),
}

impl CommandState {
    pub fn handle_input_line(&mut self, line: &str, p: ::UpdateParameters) {
        let mut tmp_state = CommandState::Idle;
        ::std::mem::swap(&mut tmp_state, self);
        *self = match tmp_state {
            CommandState::Idle => Self::dispatch_command(line, p),
            CommandState::WaitingForConfirmation(cmd) => Self::execute_if_confirmed(line, cmd, p),
        }
    }

    fn execute_if_confirmed(line: &str, cmd: Command, p: ::UpdateParameters) -> Self {
        match line {
            "y" | "Y" | "yes" => {
                Self::try_execute(cmd, p);
                CommandState::Idle
            }
            "n" | "N" | "no" => CommandState::Idle,
            _ => {
                p.message_sink.send("Please type 'y' or 'n'.");
                CommandState::WaitingForConfirmation(cmd)
            }
        }
    }

    fn print_execute_error(e: ExecuteError, p: ::UpdateParameters) {
        match e {
            ExecuteError::Quit => p.message_sink.send("quit"),
            ExecuteError::Busy => p.message_sink.send("GDB is running!"),
        }
    }

    fn try_execute(mut cmd: Command, p: ::UpdateParameters) {
        match (cmd.cmd)(p) {
            Ok(_) => {}
            Err(e) => Self::print_execute_error(e, p),
        }
    }

    fn ask_if_session_active(
        cmd: Command,
        confirmation_question: &'static str,
        p: ::UpdateParameters,
    ) -> Self {
        match p.gdb.mi.is_session_active() {
            Ok(true) => {
                p.message_sink.send(format!(
                    "A debugging session is active. {} (y or n)",
                    confirmation_question
                ));
                CommandState::WaitingForConfirmation(cmd)
            }
            Ok(false) => {
                Self::try_execute(cmd, p);
                CommandState::Idle
            }
            Err(e) => {
                Self::print_execute_error(e, p);
                CommandState::Idle
            }
        }
    }

    fn dispatch_command(line: &str, p: ::UpdateParameters) -> Self {
        let mut cmd_split = line.split(' ');
        let cmd = if let Some(cmd) = cmd_split.next() {
            cmd
        } else {
            return CommandState::Idle;
        };
        let _arguments = cmd_split.collect::<Vec<_>>();
        match cmd {
            "!stop" => {
                p.gdb.mi.interrupt_execution().expect("interrupted gdb");
                // This does not always seem to unblock gdb, but only hang it
                //gdb.execute(&MiCommand::exec_interrupt()).expect("Interrupt");

                CommandState::Idle
            }
            "!reload" => match p.gdb.get_target() {
                Ok(Some(target)) => Self::ask_if_session_active(
                    Command::from_mi_with_msg(
                        MiCommand::file_exec_and_symbols(&target),
                        "Reloaded target.",
                    ),
                    "Reload anyway?",
                    p,
                ),
                Ok(None) => {
                    p.message_sink
                        .send("No target. Use the 'file' command to specify one.");
                    CommandState::Idle
                }
                Err(e) => {
                    Self::print_execute_error(e, p);
                    CommandState::Idle
                }
            },
            "q" => {
                Self::ask_if_session_active(Command::from_mi(MiCommand::exit()), "Quit anyway?", p)
            }
            // Gdb commands
            _ => {
                match p.gdb.mi.execute(MiCommand::cli_exec(line)) {
                    Ok(ResultRecord {
                        class: ResultClass::Error,
                        results,
                        ..
                    }) => {
                        // Most of the time gdb seems to also write error messages to the console.
                        // We therefore (only) write the error message to debug log to avoid duplicates.
                        error!("{}", results["msg"].as_str().unwrap_or(&results.pretty(2)));
                    }
                    Ok(_) => {}
                    Err(e) => Self::print_execute_error(e, p),
                }
                CommandState::Idle
            }
        }
    }
}
