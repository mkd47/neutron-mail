//! Seed the GNOME "Personal" address book (evolution-data-server) with the
//! vCards from a file, using the same D-Bus client the app uses.
//! Usage: cargo run -p neutron-core --example eds_seed -- testserver/contacts.vcf
//! Run inside the user's (or a test) D-Bus session.

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args()
        .nth(1)
        .ok_or("usage: eds_seed <file.vcf>")?;
    let data = std::fs::read_to_string(path)?;
    let eds = neutron_core::gnome::Eds::connect().await?;
    let mut n = 0;
    for card in data
        .split("END:VCARD")
        .filter(|c| c.contains("BEGIN:VCARD"))
    {
        let vcard = format!("{}END:VCARD\r\n", card.trim_start());
        let email = neutron_core::gnome::parse_vcard(&vcard)
            .emails
            .first()
            .cloned()
            .unwrap_or_default();
        if !email.is_empty()
            && eds
                .search(&email, 1)
                .await?
                .iter()
                .any(|h| h.email == email)
        {
            println!("exists: {email}");
            continue;
        }
        let uid = eds.add_contact_vcard(&vcard).await?;
        println!("added {email} ({uid})");
        n += 1;
    }
    println!("{n} contact(s) added to the local address book");
    Ok(())
}
