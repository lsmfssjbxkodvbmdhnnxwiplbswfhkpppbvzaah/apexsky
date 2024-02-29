use anyhow::{anyhow, Context};
use core::time;
use memflow::prelude::v1::*;
use obfstr::obfstr as s;
use std::time::Instant;
use tracing::instrument;

use super::{MemProc, ProcessStatus};

pub struct MemflowOs {
    inventory: Inventory,
    os: OsInstanceArcBox<'static>,
}

pub struct MemflowProc<'a> {
    base_addr: Address,
    status: ProcessStatus,
    proc: ProcessInstanceArcBox<'a>,
}

impl std::fmt::Debug for MemflowOs {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        f.debug_struct("MemflowOs")
            .field(
                s!("inventory"),
                &(
                    self.inventory.available_connectors(),
                    self.inventory.available_os(),
                ),
            )
            .field(s!("os"), &self.os.info())
            .finish()
    }
}

impl<'a> std::fmt::Debug for MemflowProc<'a> {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        f.debug_struct("MemflowProc")
            .field("base_addr", &self.base_addr)
            .field("status", &self.status)
            .field(s!("proc"), &self.proc.info())
            .finish()
    }
}

impl super::MemOs for MemflowOs {
    fn new(choose_connector: &str) -> anyhow::Result<Self> {
        // load all available plugins
        let inventory = Inventory::scan();
        println!("{}", s!("inventory initialized"));

        let (connector_name, connector_args, os_name) = {
            if choose_connector == s!("kvm") {
                (
                    {
                        if std::path::Path::new(s!("/dev/memflow")).exists() {
                            s!("kvm").to_string()
                        } else {
                            s!("qemu").to_string()
                        }
                    },
                    String::new(),
                    s!("win32").to_string(),
                )
            } else if choose_connector == s!("native") {
                (String::new(), String::new(), s!("native").to_string())
            } else if choose_connector == s!("dma") {
                (
                    s!("pcileech").to_string(),
                    s!(":device=FPGA").to_string(),
                    s!("win32").to_string(),
                )
            } else {
                anyhow::bail!(format!(
                    "{}{}{}",
                    s!("no such connector `"),
                    choose_connector,
                    s!("`")
                ));
            }
        };

        let connector = if connector_name.is_empty() {
            None
        } else {
            println!("{}{}{}", s!("Using "), connector_name, s!(" connector."));

            let connector_args = if connector_args.is_empty() {
                None
            } else {
                connector_args
                    .parse()
                    .map(Some)
                    .context(s!("unable to parse connector arguments").to_string())?
            };

            inventory
                .create_connector(&connector_name, None, connector_args.as_ref())
                .map(Some)
                .context(format!(
                    "{}{}{}",
                    s!("unable to create "),
                    connector_name,
                    s!(" connector")
                ))?
        };

        let os = inventory
            .create_os(&os_name, connector, None)
            .context(format!(
                "{}{}{}{}{}",
                s!("unable to create "),
                os_name,
                s!("instance with "),
                connector_name,
                s!(" connector")
            ))?;

        Ok(MemflowOs {
            inventory: inventory.into(),
            os,
        })
    }

    #[instrument]
    fn open_proc<'a>(&'a mut self, name: String) -> anyhow::Result<super::MemProcImpl<'a>> {
        let mut proc = self.os.process_by_name(&name)?;

        let proc_info = proc.info();

        println!(
            "{}{}{:x}{}{}{}{}{}{}",
            name,
            s!(" process found: 0x"),
            proc_info.address,
            s!("] "),
            proc_info.pid,
            s!(" "),
            proc_info.name,
            s!(" "),
            proc_info.path
        );

        let module_info = proc.module_by_name(&name)?;

        println!(
            "{}{}{:x}{}{:x}{}{}{}{}",
            name,
            s!(" module found: 0x"),
            module_info.address,
            s!("] 0x"),
            module_info.base,
            s!(" "),
            module_info.name,
            s!(" "),
            module_info.path
        );

        Ok(super::MemProcImpl::Memflow(MemflowProc {
            base_addr: module_info.base,
            status: ProcessStatus::FoundReady,
            proc,
        }))
    }
}

impl<'a> MemProc for MemflowProc<'a> {
    #[inline]
    fn get_proc_baseaddr(&self) -> u64 {
        self.base_addr.to_umem()
    }

    #[instrument]
    fn check_proc_status(&mut self) -> ProcessStatus {
        if self.base_addr.is_null() {
            self.status = ProcessStatus::NotFound;
            return self.status;
        }

        if self.status == ProcessStatus::FoundReady {
            let mut c: i16 = 0;
            self.read_into(self.base_addr.to_umem(), &mut c).ok();

            if c != 0x5A4D {
                self.status = ProcessStatus::NotFound;
                self.base_addr = Address::null();
            }
        }

        self.status
    }

    #[instrument(skip_all)]
    fn speed_test(&mut self) {
        tracing::debug!("{}", s!("029194cf-ce9a-42aa-91c7-e35108e9ddb0"));
        if self.status != ProcessStatus::FoundReady {
            let err = anyhow!(s!("proc instance is None").to_string());
            tracing::error!(%err);
            return;
        }
        let proc = &mut self.proc;

        println!("{}", s!("Received metadata:"));
        let metadata = proc.metadata();
        tracing::debug!(?metadata);
        println!("{}{:x}", s!("max_address=0x"), metadata.max_address);
        println!("{}{:x}", s!("real_size=0x"), metadata.real_size);
        println!("{}{}", s!("readonly="), metadata.readonly);

        println!("{}", s!("== speed test start =="));

        let addr = self.base_addr;
        let start = Instant::now();
        for counter in 0..5000 {
            let mut buf = vec![0; 0x1000];
            proc.read_raw_into(addr, &mut buf)
                .expect(s!("speed_test: unable to read process memory"));

            if counter % 1000 == 0 {
                let elapsed = start.elapsed().as_millis() as f64;
                if elapsed > 0.0 {
                    let result = (
                        format!(
                            "{}{}",
                            (f64::from(counter)) / elapsed * 1000.0,
                            s!(" reads/sec")
                        ),
                        format!("{}{}", elapsed / (f64::from(counter)), s!(" ms/read")),
                    );
                    tracing::debug!(result0 = result.0, result1 = result.1);
                    println!("{}", result.0);
                    println!("{}", result.1);
                }
            }
        }

        println!("{}", s!("== speed test end =="));
    }

    #[instrument(skip_all)]
    fn read_into<T: Pod + ?Sized>(&mut self, addr: u64, out: &mut T) -> anyhow::Result<()> {
        if self.status != ProcessStatus::FoundReady {
            anyhow::bail!(s!("proc instance is None").to_string());
        }

        let addr = Address::from(addr);
        let dest = dataview::bytes_mut(out);

        let mut result = Ok(());
        for i in 0..3 {
            result = self.proc.read_raw_into(addr, dest);
            match &result {
                Ok(_) => {
                    return Ok(());
                }
                Err(e) => {
                    tracing::debug!(%e, retry=i);
                    if i > 1 {
                        std::thread::sleep(time::Duration::from_millis(2));
                    }
                }
            }
        }
        // if let Err(e) = result {
        //     tracing::warn!(%e);
        // }
        //Ok(())
        Ok(result?)
    }

    #[instrument(skip_all)]
    fn write<T: Pod + ?Sized>(&mut self, addr: u64, data: &T) -> anyhow::Result<()> {
        if self.status != ProcessStatus::FoundReady {
            anyhow::bail!(s!("proc instance is None").to_string());
        }

        let addr = Address::from(addr);
        let data = dataview::bytes(data);

        let mut result = Ok(());
        for i in 0..3 {
            result = self.proc.write_raw(addr, data);
            match &result {
                Ok(_) => {
                    return Ok(());
                }
                Err(e) => {
                    tracing::debug!(%e, retry=i);
                    if i > 1 {
                        std::thread::sleep(time::Duration::from_millis(2));
                    }
                }
            }
        }
        Ok(result?)
        // if let Err(e) = result {
        //     tracing::warn!(%e);
        // }
        //Ok(())
    }
}