use bip39::{Language, Mnemonic};
use bitcoin::bip32::Xpriv;
use bitcoin::key::UntweakedPublicKey;
use bitcoin::secp256k1::All;
use bitcoin::{secp256k1::Secp256k1, Address, CompressedPublicKey, Network, PrivateKey, PublicKey};
use clap::Parser;
use std::{
    collections::HashSet,
    fs::{File, OpenOptions},
    io::{self, BufRead, Write},
    path::Path,
    sync::{mpsc, Arc},
    thread,
    time::Instant,
};

/// Searches for a private key corresponding to a list of Bitcoin addresses.
#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Number of CPU cores to use
    #[arg(short, long, default_value_t = 4)]
    cores: u32,

    /// File containing BTC addresses
    #[arg(short, long, default_value = "Bitcoin_addresses_LATEST.txt")]
    addresses: String,

    /// File to output found keys
    #[arg(short, long, default_value = "found_keys.txt")]
    keyfile: String,
}

fn read_lines<P>(filename: P) -> io::Result<io::Lines<io::BufReader<File>>>
where
    P: AsRef<Path>,
{
    let file = File::open(filename)?;
    Ok(io::BufReader::new(file).lines())
}

// Structure to hold different address types
#[derive(Debug)]
struct AddressSet {
    p2pkh: Address,
    p2wpkh: Address,
    p2shwpkh: Address,
    p2tr: Address,
}

fn generate_addresses_from_mnemonic(
    secp: &Secp256k1<All>,
    network: Network,
) -> Option<(PrivateKey, AddressSet)> {
    let mnemonic = Mnemonic::generate_in(Language::English, 24).unwrap();

    // Generate seed from mnemonic
    let seed = mnemonic.to_seed("");

    // Create master private key from seed
    let master_private_key = Xpriv::new_master(network, &seed).ok()?;

    // Get the secret key
    let private_key = master_private_key.to_priv();

    // Generate public key
    let public_key = PublicKey::from_private_key(&secp, &private_key);

    // Generate compressed public key
    let compressed_pub_key = CompressedPublicKey::from_private_key(&secp, &private_key)
        .expect("Unable to generate compressed public key from the private key");

    // Generate different address types
    let p2pkh = Address::p2pkh(&public_key, network);
    let p2wpkh = Address::p2wpkh(&compressed_pub_key, network);
    let p2shwpkh = Address::p2shwpkh(&compressed_pub_key, network);

    // For P2TR (Taproot), we need to create a taproot key
    let p2tr = Address::p2tr(
        secp,
        UntweakedPublicKey::from(compressed_pub_key),
        None,
        network,
    );

    let address_set = AddressSet {
        p2pkh,
        p2wpkh,
        p2shwpkh,
        p2tr,
    };

    Some((private_key, address_set))
}

fn seek(core: u32, tx: mpsc::Sender<(PrivateKey, AddressSet)>) {
    println!("Core {}: Searching for Private Key...", core);
    let log_rate_iterations = 10000;
    let start_time = Instant::now();
    let mut iterations = 0;

    let secp = Secp256k1::new();
    let network = Network::Bitcoin;

    loop {
        iterations += 1;
        // Generate mnemonic and derive addresses

        if let Some((private_key, address_set)) = generate_addresses_from_mnemonic(&secp, network) {
            if tx.send((private_key, address_set)).is_err() {
                // Main thread has hung up.
                break;
            }
        }

        // log rate
        if (iterations % log_rate_iterations) == 0 {
            let time_diff = start_time.elapsed().as_secs_f64();
            if time_diff > 0.0 {
                println!("Core {}: {:.2} Key/s", core, iterations as f64 / time_diff);
            }
        }
    }
}

fn main() {
    let args = Args::parse();

    // generate list of pubkey with BTC
    println!("Loading \"{}\"...", &args.addresses);

    let address_list: Arc<HashSet<String>> = if let Ok(lines) = read_lines(args.addresses) {
        Arc::new(lines.map(|line| line.unwrap_or_default()).collect())
    } else {
        eprintln!("Error reading addresses file. Exiting.");
        return;
    };

    println!("Loaded.");

    let (tx, rx) = mpsc::channel();

    for core in 0..args.cores {
        let tx_clone = tx.clone();
        thread::spawn(move || {
            seek(core, tx_clone);
        });
    }
    // Drop the original sender so the channel closes when all threads are done.
    drop(tx);

    let mut key_output_file = OpenOptions::new()
        .append(true)
        .create(true)
        .open(&args.keyfile)
        .expect("Could not open or create keyfile.");

    for (private_key, address_set) in rx {
        // Check all address types
        let p2pkh_str = address_set.p2pkh.to_string();
        let p2wpkh_str = address_set.p2wpkh.to_string();
        let p2shwpkh_str = address_set.p2shwpkh.to_string();
        let p2tr_str = address_set.p2tr.to_string();

        if address_list.contains(&p2pkh_str)
            || address_list.contains(&p2wpkh_str)
            || address_list.contains(&p2shwpkh_str)
            || address_list.contains(&p2tr_str)
        {
            let found_key = format!(
                "\nPrivate: {:?} | P2PKH: {} | P2WPKH: {} | P2SHWPKH: {} | P2TR: {}\n",
                private_key, p2pkh_str, p2wpkh_str, p2shwpkh_str, p2tr_str
            );
            print!("{}", found_key);
            if let Err(e) = key_output_file.write_all(found_key.as_bytes()) {
                eprintln!("Couldn't write to file {}: {}", args.keyfile, e);
            }
        }
    }
}
