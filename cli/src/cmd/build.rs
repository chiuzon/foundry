//! build command

use ethers::{
    solc::{
        remappings::Remapping, MinimalCombinedArtifacts, Project, ProjectCompileOutput,
        ProjectPathsConfig,
    },
    types::Address,
};
use std::{path::PathBuf, str::FromStr};

use crate::{utils::find_git_root_path, Cmd};
#[cfg(feature = "evmodin-evm")]
use evmodin::util::mocked_host::MockedHost;
#[cfg(feature = "evmodin-evm")]
use evmodin::Revision;
#[cfg(feature = "sputnik-evm")]
use sputnik::backend::MemoryVicinity;
#[cfg(feature = "sputnik-evm")]
use sputnik::Config;
use structopt::StructOpt;

#[derive(Debug, Clone, StructOpt)]
pub struct BuildArgs {
    #[structopt(
        help = "the project's root path, default being the current working directory",
        long
    )]
    pub root: Option<PathBuf>,

    #[structopt(
        help = "the directory relative to the root under which the smart contrats are",
        long,
        short
    )]
    #[structopt(env = "DAPP_SRC")]
    pub contracts: Option<PathBuf>,

    #[structopt(help = "the remappings", long, short)]
    pub remappings: Vec<ethers::solc::remappings::Remapping>,
    #[structopt(long = "remappings-env", env = "DAPP_REMAPPINGS")]
    pub remappings_env: Option<String>,

    #[structopt(help = "the paths where your libraries are installed", long)]
    pub lib_paths: Vec<PathBuf>,

    #[structopt(help = "path to where the contract artifacts are stored", long = "out", short)]
    pub out_path: Option<PathBuf>,

    #[structopt(help = "choose the evm version", long, default_value = "london")]
    pub evm_version: EvmVersion,

    #[structopt(
        help = "if set to true, skips auto-detecting solc and uses what is in the user's $PATH ",
        long
    )]
    pub no_auto_detect: bool,

    #[structopt(
        help = "force recompilation of the project, deletes the cache and artifacts folders",
        long
    )]
    pub force: bool,
}

impl Cmd for BuildArgs {
    type Output = ProjectCompileOutput<MinimalCombinedArtifacts>;
    fn run(self) -> eyre::Result<Self::Output> {
        let project = Project::try_from(&self)?;
        let output = project.compile()?;
        if output.has_compiler_errors() {
            // return the diagnostics error back to the user.
            eyre::bail!(output.to_string())
        } else if output.is_unchanged() {
            println!("no files changed, compilation skippped.");
        } else {
            println!("success.");
        }
        Ok(output)
    }
}

impl std::convert::TryFrom<&BuildArgs> for Project {
    type Error = eyre::Error;

    /// Defaults to converting to DAppTools-style repo layout, but can be customized.
    fn try_from(opts: &BuildArgs) -> eyre::Result<Project> {
        // 1. Set the root dir
        let root = opts.root.clone().unwrap_or_else(|| {
            find_git_root_path().unwrap_or_else(|_| std::env::current_dir().unwrap())
        });
        let root = std::fs::canonicalize(&root)?;

        // 2. Set the contracts dir
        let contracts = if let Some(ref contracts) = opts.contracts {
            root.join(contracts)
        } else {
            root.join("src")
        };

        // 3. Set the output dir
        let artifacts = if let Some(ref artifacts) = opts.out_path {
            root.join(artifacts)
        } else {
            root.join("out")
        };

        // 4. Set where the libraries are going to be read from
        // default to the lib path being the `lib/` dir
        let lib_paths =
            if opts.lib_paths.is_empty() { vec![root.join("lib")] } else { opts.lib_paths.clone() };

        // get all the remappings corresponding to the lib paths
        let mut remappings: Vec<_> =
            lib_paths.iter().map(|path| Remapping::find_many(&path).unwrap()).flatten().collect();

        // extend them with the once manually provided in the opts
        remappings.extend_from_slice(&opts.remappings);

        // extend them with the one via the env vars
        if let Some(ref env) = opts.remappings_env {
            remappings.extend(remappings_from_newline(env))
        }

        // extend them with the one via the requirements.txt
        if let Ok(ref remap) = std::fs::read_to_string(root.join("remappings.txt")) {
            remappings.extend(remappings_from_newline(remap))
        }

        // helper function for parsing newline-separated remappings
        fn remappings_from_newline(remappings: &str) -> impl Iterator<Item = Remapping> + '_ {
            remappings.split('\n').filter(|x| !x.is_empty()).map(|x| {
                Remapping::from_str(x)
                    .unwrap_or_else(|_| panic!("could not parse remapping: {}", x))
            })
        }

        // remove any potential duplicates
        remappings.sort_unstable();
        remappings.dedup();

        // build the path
        let mut paths_builder =
            ProjectPathsConfig::builder().root(&root).sources(contracts).artifacts(artifacts);

        if !remappings.is_empty() {
            paths_builder = paths_builder.remappings(remappings);
        }

        let paths = paths_builder.build()?;

        // build the project w/ allowed paths = root and all the libs
        let mut builder =
            Project::builder().paths(paths).allowed_path(&root).allowed_paths(lib_paths);

        if opts.no_auto_detect {
            builder = builder.no_auto_detect();
        }

        let project = builder.build()?;

        // if `--force` is provided, it proceeds to remove the cache
        // and recompile the contracts.
        if opts.force {
            project.cleanup()?;
        }

        Ok(project)
    }
}

#[derive(Clone, Debug)]
pub enum EvmType {
    #[cfg(feature = "sputnik-evm")]
    Sputnik,
    #[cfg(feature = "evmodin-evm")]
    EvmOdin,
}

impl FromStr for EvmType {
    type Err = eyre::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s.to_lowercase().as_str() {
            #[cfg(feature = "sputnik-evm")]
            "sputnik" => EvmType::Sputnik,
            #[cfg(feature = "evmodin-evm")]
            "evmodin" => EvmType::EvmOdin,
            other => eyre::bail!("unknown EVM type {}", other),
        })
    }
}

#[derive(Clone, Debug)]
pub enum EvmVersion {
    Frontier,
    Istanbul,
    Berlin,
    London,
}

impl EvmVersion {
    #[cfg(feature = "sputnik-evm")]
    pub fn sputnik_cfg(self) -> Config {
        use EvmVersion::*;
        match self {
            Frontier => Config::frontier(),
            Istanbul => Config::istanbul(),
            Berlin => Config::berlin(),
            London => Config::london(),
        }
    }

    #[cfg(feature = "evmodin-evm")]
    pub fn evmodin_cfg(self) -> Revision {
        use EvmVersion::*;
        match self {
            Frontier => Revision::Frontier,
            Istanbul => Revision::Istanbul,
            Berlin => Revision::Berlin,
            London => Revision::London,
        }
    }
}

impl FromStr for EvmVersion {
    type Err = eyre::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        use EvmVersion::*;
        Ok(match s.to_lowercase().as_str() {
            "frontier" => Frontier,
            "istanbul" => Istanbul,
            "berlin" => Berlin,
            "london" => London,
            _ => eyre::bail!("unsupported evm version: {}", s),
        })
    }
}

#[derive(Debug, Clone, StructOpt)]
pub struct Env {
    // structopt does not let use `u64::MAX`:
    // https://doc.rust-lang.org/std/primitive.u64.html#associatedconstant.MAX
    #[structopt(help = "the block gas limit", long, default_value = "18446744073709551615")]
    pub gas_limit: u64,

    #[structopt(help = "the chainid opcode value", long, default_value = "1")]
    pub chain_id: u64,

    #[structopt(help = "the tx.gasprice value during EVM execution", long, default_value = "0")]
    pub gas_price: u64,

    #[structopt(help = "the base fee in a block", long, default_value = "0")]
    pub block_base_fee_per_gas: u64,

    #[structopt(
        help = "the tx.origin value during EVM execution",
        long,
        default_value = "0x0000000000000000000000000000000000000000"
    )]
    pub tx_origin: Address,

    #[structopt(
    help = "the block.coinbase value during EVM execution",
    long,
    // TODO: It'd be nice if we could use Address::zero() here.
    default_value = "0x0000000000000000000000000000000000000000"
    )]
    pub block_coinbase: Address,
    #[structopt(
        help = "the block.timestamp value during EVM execution",
        long,
        default_value = "0",
        env = "DAPP_TEST_TIMESTAMP"
    )]
    pub block_timestamp: u64,

    #[structopt(help = "the block.number value during EVM execution", long, default_value = "0")]
    #[structopt(env = "DAPP_TEST_NUMBER")]
    pub block_number: u64,

    #[structopt(
        help = "the block.difficulty value during EVM execution",
        long,
        default_value = "0"
    )]
    pub block_difficulty: u64,

    #[structopt(help = "the block.gaslimit value during EVM execution", long)]
    pub block_gas_limit: Option<u64>,
    // TODO: Add configuration option for base fee.
}

impl Env {
    #[cfg(feature = "sputnik-evm")]
    pub fn sputnik_state(&self) -> MemoryVicinity {
        MemoryVicinity {
            chain_id: self.chain_id.into(),

            gas_price: self.gas_price.into(),
            origin: self.tx_origin,

            block_coinbase: self.block_coinbase,
            block_number: self.block_number.into(),
            block_timestamp: self.block_timestamp.into(),
            block_difficulty: self.block_difficulty.into(),
            block_base_fee_per_gas: self.block_base_fee_per_gas.into(),
            block_gas_limit: self.block_gas_limit.unwrap_or(self.gas_limit).into(),
            block_hashes: Vec::new(),
        }
    }

    #[cfg(feature = "evmodin-evm")]
    pub fn evmodin_state(&self) -> MockedHost {
        let mut host = MockedHost::default();

        host.tx_context.chain_id = self.chain_id.into();
        host.tx_context.tx_gas_price = self.gas_price.into();
        host.tx_context.tx_origin = self.tx_origin;
        host.tx_context.block_coinbase = self.block_coinbase;
        host.tx_context.block_number = self.block_number;
        host.tx_context.block_timestamp = self.block_timestamp;
        host.tx_context.block_difficulty = self.block_difficulty.into();
        host.tx_context.block_gas_limit = self.block_gas_limit.unwrap_or(self.gas_limit);

        host
    }
}
