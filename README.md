# Secure Contacts

A simple **command-line contact manager** built in Rust.  
This project was developed as part of a group assignment on secure coding practices.

## Features

- Add new contacts (with name and email)  
- List all contacts with a unique ID  
- Store contacts in a local JSON file (`contacts.json`)  
- Simple and lightweight CLI interface  

## Installation

Clone the repository:

```bash
git clone https://github.com/your-username/secure_contacts.git
cd secure_contacts
```

## Build the project:
cargo build

### Or run directly:
cargo run -- help

## Usage
### Add a new contact:
cargo run -- add "Alice" "alice@example.com"

### List all contacts:
cargo run -- list

## Example:
$ cargo run -- add "Charlie" "charlie@example.com"
Adding contact: Charlie <charlie@example.com>
Saved.

$ cargo run -- list
ddeab3cc-4256-4596-bff7-bc9d940e195f | Charlie | charlie@example.com
Total: 1

--

## Requirements:

Rust (latest stable version)

Cargo (comes with Rust)

--

## License

This project is for educational purposes and is distributed under the MIT License.
