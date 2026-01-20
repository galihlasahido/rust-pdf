//! Example demonstrating PDF form fields.
//!
//! This example creates PDFs with interactive form fields:
//! - Text fields for user input
//! - Checkboxes for boolean options
//! - Radio buttons for single-choice selections
//! - Combo boxes (dropdown lists)
//! - List boxes for multiple selection
//! - Push buttons
//!
//! Run with: cargo run --example forms_example

use rust_pdf::prelude::*;
use rust_pdf::forms::{
    TextField, CheckBox, RadioButton, RadioGroup, ComboBox, ListBox, PushButton, BorderStyle,
};
use std::fs;
use std::path::Path;

const OUTPUT_DIR: &str = "tests/output/forms";

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Create output directory
    fs::create_dir_all(OUTPUT_DIR)?;

    // Run all examples
    create_simple_form()?;
    create_registration_form()?;
    create_survey_form()?;

    println!("\nAll forms examples created successfully!");
    println!("Output directory: {}", OUTPUT_DIR);

    Ok(())
}

/// Creates a simple form with basic field types.
fn create_simple_form() -> Result<(), Box<dyn std::error::Error>> {
    let output_path = Path::new(OUTPUT_DIR).join("simple_form.pdf");
    println!("Creating simple form: {}", output_path.display());

    // Create content for the page
    let content = ContentBuilder::new()
        // Title
        .text("F1", 18.0, 72.0, 750.0, "Simple Form Example")
        // Labels
        .text("F1", 12.0, 72.0, 680.0, "Name:")
        .text("F1", 12.0, 72.0, 640.0, "Email:")
        .text("F1", 12.0, 72.0, 600.0, "Subscribe to newsletter:")
        .text("F1", 12.0, 72.0, 560.0, "Country:");

    // Create text fields
    let name_field = TextField::new("name")
        .rect(150.0, 670.0, 250.0, 20.0)
        .background_color(Color::WHITE)
        .border_color(Color::BLACK)
        .font_size(12.0);

    let email_field = TextField::new("email")
        .rect(150.0, 630.0, 250.0, 20.0)
        .background_color(Color::WHITE)
        .border_color(Color::BLACK)
        .font_size(12.0);

    // Create checkbox
    let subscribe_checkbox = CheckBox::new("subscribe")
        .rect(150.0, 595.0, 15.0, 15.0)
        .background_color(Color::WHITE)
        .border_color(Color::BLACK)
        .checked(true);

    // Create combo box
    let country_combo = ComboBox::new("country")
        .rect(150.0, 550.0, 150.0, 20.0)
        .options(vec!["United States", "Canada", "United Kingdom", "Australia", "Other"])
        .selected_index(0)
        .background_color(Color::WHITE)
        .border_color(Color::BLACK);

    // Build the page with form fields
    let page = PageBuilder::a4()
        .font("F1", Font::helvetica())
        .content(content)
        .text_field(name_field)
        .text_field(email_field)
        .checkbox(subscribe_checkbox)
        .combo_box(country_combo)
        .build();

    // Build and save the document
    let doc = DocumentBuilder::new()
        .title("Simple Form Example")
        .author("rust-pdf")
        .page(page)
        .build()?;

    doc.save_to_file(&output_path)?;
    println!("  Created: {}", output_path.display());

    Ok(())
}

/// Creates a registration form with various field types.
fn create_registration_form() -> Result<(), Box<dyn std::error::Error>> {
    let output_path = Path::new(OUTPUT_DIR).join("registration_form.pdf");
    println!("Creating registration form: {}", output_path.display());

    let content = ContentBuilder::new()
        // Header
        .fill_color(Color::rgb(0.2, 0.4, 0.8))
        .rect(0.0, 780.0, 595.0, 62.0)
        .fill()
        .fill_color(Color::WHITE)
        .text("F1", 24.0, 72.0, 800.0, "User Registration")
        .fill_color(Color::BLACK)
        // Personal Information Section
        .text("F1", 14.0, 72.0, 720.0, "Personal Information")
        .stroke_color(Color::gray(0.7))
        .line_width(0.5)
        .move_to(72.0, 715.0)
        .line_to(523.0, 715.0)
        .stroke()
        // Labels
        .text("F1", 11.0, 72.0, 680.0, "First Name:")
        .text("F1", 11.0, 300.0, 680.0, "Last Name:")
        .text("F1", 11.0, 72.0, 640.0, "Email Address:")
        .text("F1", 11.0, 72.0, 600.0, "Phone Number:")
        .text("F1", 11.0, 72.0, 560.0, "Date of Birth:")
        // Preferences Section
        .text("F1", 14.0, 72.0, 500.0, "Preferences")
        .move_to(72.0, 495.0)
        .line_to(523.0, 495.0)
        .stroke()
        .text("F1", 11.0, 72.0, 460.0, "Gender:")
        .text("F1", 11.0, 150.0, 460.0, "Male")
        .text("F1", 11.0, 220.0, 460.0, "Female")
        .text("F1", 11.0, 290.0, 460.0, "Other")
        .text("F1", 11.0, 72.0, 420.0, "Interests:")
        .text("F1", 11.0, 72.0, 340.0, "Communication Preferences:")
        .text("F1", 10.0, 92.0, 310.0, "Email updates")
        .text("F1", 10.0, 92.0, 290.0, "SMS notifications")
        .text("F1", 10.0, 92.0, 270.0, "Phone calls");

    // Form fields
    let first_name = TextField::new("first_name")
        .rect(150.0, 670.0, 130.0, 20.0)
        .background_color(Color::gray(0.95))
        .border_color(Color::gray(0.7))
        .border_style(BorderStyle::Solid)
        .font_size(11.0);

    let last_name = TextField::new("last_name")
        .rect(380.0, 670.0, 140.0, 20.0)
        .background_color(Color::gray(0.95))
        .border_color(Color::gray(0.7))
        .font_size(11.0);

    let email = TextField::new("email")
        .rect(150.0, 630.0, 250.0, 20.0)
        .background_color(Color::gray(0.95))
        .border_color(Color::gray(0.7))
        .font_size(11.0);

    let phone = TextField::new("phone")
        .rect(150.0, 590.0, 150.0, 20.0)
        .background_color(Color::gray(0.95))
        .border_color(Color::gray(0.7))
        .font_size(11.0);

    let dob = TextField::new("dob")
        .rect(150.0, 550.0, 100.0, 20.0)
        .background_color(Color::gray(0.95))
        .border_color(Color::gray(0.7))
        .default_value("MM/DD/YYYY")
        .font_size(11.0);

    // Radio buttons for gender
    let gender_group = RadioGroup::new("gender")
        .add_button(RadioButton::new("male").rect(130.0, 455.0, 15.0, 15.0))
        .add_button(RadioButton::new("female").rect(200.0, 455.0, 15.0, 15.0))
        .add_button(RadioButton::new("other").rect(270.0, 455.0, 15.0, 15.0))
        .background_color(Color::WHITE)
        .border_color(Color::gray(0.5));

    // List box for interests
    let interests = ListBox::new("interests")
        .rect(150.0, 370.0, 180.0, 70.0)
        .options(vec![
            "Technology",
            "Sports",
            "Music",
            "Travel",
            "Reading",
            "Gaming",
        ])
        .multi_select()
        .background_color(Color::WHITE)
        .border_color(Color::gray(0.7))
        .font_size(10.0);

    // Checkboxes for communication preferences
    let email_updates = CheckBox::new("email_updates")
        .rect(72.0, 305.0, 15.0, 15.0)
        .background_color(Color::WHITE)
        .border_color(Color::gray(0.5))
        .checked(true);

    let sms_notifications = CheckBox::new("sms_notifications")
        .rect(72.0, 285.0, 15.0, 15.0)
        .background_color(Color::WHITE)
        .border_color(Color::gray(0.5));

    let phone_calls = CheckBox::new("phone_calls")
        .rect(72.0, 265.0, 15.0, 15.0)
        .background_color(Color::WHITE)
        .border_color(Color::gray(0.5));

    // Submit button
    let submit_button = PushButton::new("submit")
        .rect(200.0, 200.0, 100.0, 30.0)
        .caption("Register")
        .background_color(Color::rgb(0.2, 0.6, 0.2))
        .border_color(Color::rgb(0.1, 0.4, 0.1))
        .text_color(Color::WHITE)
        .border_style(BorderStyle::Beveled)
        .font_size(12.0);

    // Clear button
    let clear_button = PushButton::new("clear")
        .rect(320.0, 200.0, 100.0, 30.0)
        .caption("Clear Form")
        .background_color(Color::gray(0.8))
        .border_color(Color::gray(0.5))
        .border_style(BorderStyle::Beveled)
        .font_size(12.0);

    let page = PageBuilder::a4()
        .font("F1", Font::helvetica())
        .content(content)
        .text_field(first_name)
        .text_field(last_name)
        .text_field(email)
        .text_field(phone)
        .text_field(dob)
        .radio_group(gender_group)
        .list_box(interests)
        .checkbox(email_updates)
        .checkbox(sms_notifications)
        .checkbox(phone_calls)
        .push_button(submit_button)
        .push_button(clear_button)
        .build();

    let doc = DocumentBuilder::new()
        .title("User Registration Form")
        .author("rust-pdf")
        .page(page)
        .build()?;

    doc.save_to_file(&output_path)?;
    println!("  Created: {}", output_path.display());

    Ok(())
}

/// Creates a survey form demonstrating various field styles.
fn create_survey_form() -> Result<(), Box<dyn std::error::Error>> {
    let output_path = Path::new(OUTPUT_DIR).join("survey_form.pdf");
    println!("Creating survey form: {}", output_path.display());

    let content = ContentBuilder::new()
        // Header
        .fill_color(Color::rgb(0.6, 0.2, 0.6))
        .rect(0.0, 780.0, 595.0, 62.0)
        .fill()
        .fill_color(Color::WHITE)
        .text("F1", 22.0, 72.0, 800.0, "Customer Satisfaction Survey")
        .fill_color(Color::BLACK)
        // Introduction
        .text("F1", 11.0, 72.0, 730.0, "Thank you for taking the time to complete this survey.")
        .text("F1", 11.0, 72.0, 715.0, "Your feedback helps us improve our services.")
        // Question 1
        .text("F1", 12.0, 72.0, 670.0, "1. How satisfied are you with our service?")
        .text("F1", 10.0, 92.0, 645.0, "Very Satisfied")
        .text("F1", 10.0, 92.0, 625.0, "Satisfied")
        .text("F1", 10.0, 92.0, 605.0, "Neutral")
        .text("F1", 10.0, 92.0, 585.0, "Dissatisfied")
        .text("F1", 10.0, 92.0, 565.0, "Very Dissatisfied")
        // Question 2
        .text("F1", 12.0, 72.0, 520.0, "2. Which of our products do you use? (Select all that apply)")
        .text("F1", 10.0, 92.0, 495.0, "Product A")
        .text("F1", 10.0, 92.0, 475.0, "Product B")
        .text("F1", 10.0, 92.0, 455.0, "Product C")
        .text("F1", 10.0, 92.0, 435.0, "Product D")
        // Question 3
        .text("F1", 12.0, 72.0, 390.0, "3. How did you hear about us?")
        // Question 4
        .text("F1", 12.0, 72.0, 320.0, "4. Please provide any additional comments:")
        // Question 5
        .text("F1", 12.0, 72.0, 200.0, "5. Would you recommend us to others?")
        .text("F1", 10.0, 92.0, 175.0, "Definitely yes")
        .text("F1", 10.0, 92.0, 155.0, "Probably yes")
        .text("F1", 10.0, 92.0, 135.0, "Not sure")
        .text("F1", 10.0, 92.0, 115.0, "Probably not")
        .text("F1", 10.0, 92.0, 95.0, "Definitely not");

    // Question 1: Satisfaction rating (radio buttons)
    let satisfaction_group = RadioGroup::new("satisfaction")
        .add_button(RadioButton::new("very_satisfied").rect(72.0, 640.0, 15.0, 15.0))
        .add_button(RadioButton::new("satisfied").rect(72.0, 620.0, 15.0, 15.0))
        .add_button(RadioButton::new("neutral").rect(72.0, 600.0, 15.0, 15.0))
        .add_button(RadioButton::new("dissatisfied").rect(72.0, 580.0, 15.0, 15.0))
        .add_button(RadioButton::new("very_dissatisfied").rect(72.0, 560.0, 15.0, 15.0))
        .background_color(Color::WHITE)
        .border_color(Color::rgb(0.6, 0.2, 0.6));

    // Question 2: Products (checkboxes)
    let product_a = CheckBox::new("product_a")
        .rect(72.0, 490.0, 15.0, 15.0)
        .background_color(Color::WHITE)
        .border_color(Color::gray(0.5));

    let product_b = CheckBox::new("product_b")
        .rect(72.0, 470.0, 15.0, 15.0)
        .background_color(Color::WHITE)
        .border_color(Color::gray(0.5));

    let product_c = CheckBox::new("product_c")
        .rect(72.0, 450.0, 15.0, 15.0)
        .background_color(Color::WHITE)
        .border_color(Color::gray(0.5));

    let product_d = CheckBox::new("product_d")
        .rect(72.0, 430.0, 15.0, 15.0)
        .background_color(Color::WHITE)
        .border_color(Color::gray(0.5));

    // Question 3: How did you hear about us (combo box)
    let referral_source = ComboBox::new("referral_source")
        .rect(72.0, 355.0, 200.0, 20.0)
        .options(vec![
            "Search Engine",
            "Social Media",
            "Friend/Family",
            "Advertisement",
            "Other",
        ])
        .background_color(Color::WHITE)
        .border_color(Color::gray(0.5))
        .font_size(10.0);

    // Question 4: Comments (multiline text field)
    let comments = TextField::new("comments")
        .rect(72.0, 220.0, 450.0, 80.0)
        .multiline()
        .background_color(Color::WHITE)
        .border_color(Color::gray(0.5))
        .font_size(10.0);

    // Question 5: Recommendation (radio buttons)
    let recommendation_group = RadioGroup::new("recommendation")
        .add_button(RadioButton::new("definitely_yes").rect(72.0, 170.0, 15.0, 15.0))
        .add_button(RadioButton::new("probably_yes").rect(72.0, 150.0, 15.0, 15.0))
        .add_button(RadioButton::new("not_sure").rect(72.0, 130.0, 15.0, 15.0))
        .add_button(RadioButton::new("probably_not").rect(72.0, 110.0, 15.0, 15.0))
        .add_button(RadioButton::new("definitely_not").rect(72.0, 90.0, 15.0, 15.0))
        .background_color(Color::WHITE)
        .border_color(Color::rgb(0.6, 0.2, 0.6));

    // Submit button
    let submit_button = PushButton::new("submit_survey")
        .rect(230.0, 40.0, 120.0, 35.0)
        .caption("Submit Survey")
        .background_color(Color::rgb(0.6, 0.2, 0.6))
        .border_color(Color::rgb(0.4, 0.1, 0.4))
        .text_color(Color::WHITE)
        .border_style(BorderStyle::Beveled)
        .font_size(12.0);

    let page = PageBuilder::a4()
        .font("F1", Font::helvetica())
        .content(content)
        .radio_group(satisfaction_group)
        .checkbox(product_a)
        .checkbox(product_b)
        .checkbox(product_c)
        .checkbox(product_d)
        .combo_box(referral_source)
        .text_field(comments)
        .radio_group(recommendation_group)
        .push_button(submit_button)
        .build();

    let doc = DocumentBuilder::new()
        .title("Customer Satisfaction Survey")
        .author("rust-pdf")
        .page(page)
        .build()?;

    doc.save_to_file(&output_path)?;
    println!("  Created: {}", output_path.display());

    Ok(())
}
