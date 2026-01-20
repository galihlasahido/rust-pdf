#!/usr/bin/env ruby
# Example: Using rust-pdf library from Ruby
#
# Prerequisites:
#   gem install ffi
#
# Run with:
#   # macOS
#   DYLD_LIBRARY_PATH=../target/release ruby ruby_example.rb
#
#   # Linux
#   LD_LIBRARY_PATH=../target/release ruby ruby_example.rb

require 'ffi'

module RustPdf
  extend FFI::Library

  # Load the appropriate library based on platform
  case RbConfig::CONFIG['host_os']
  when /darwin/
    ffi_lib 'librust_pdf.dylib'
  when /mswin|mingw/
    ffi_lib 'rust_pdf.dll'
  else
    ffi_lib 'librust_pdf.so'
  end

  # Define functions
  attach_function :pdf_version, [], :string
  attach_function :pdf_create_simple, [:string, :double], :pointer
  attach_function :pdf_get_data, [:pointer, :pointer], :size_t
  attach_function :pdf_save_to_file, [:pointer, :string], :int
  attach_function :pdf_free, [:pointer], :void
end

puts "rust-pdf Ruby Example"
puts "=====================\n\n"

# Get version
version = RustPdf.pdf_version
puts "Library version: #{version}\n\n"

# Create PDF
text = "Hello from Ruby!\n\nThis PDF was created using rust-pdf via FFI."
puts "Creating PDF..."
pdf = RustPdf.pdf_create_simple(text, 18.0)

if pdf.null?
  puts "Error: Failed to create PDF"
  exit 1
end

# Get PDF size
data_ptr = FFI::MemoryPointer.new(:pointer)
size = RustPdf.pdf_get_data(pdf, data_ptr)
puts "PDF size: #{size} bytes"

# Save to file
filename = "hello_from_ruby.pdf"
puts "Saving to '#{filename}'..."
result = RustPdf.pdf_save_to_file(pdf, filename)

if result == 0
  puts "Success! PDF saved."
else
  puts "Error: Failed to save PDF"
end

# Free resources
RustPdf.pdf_free(pdf)
puts "\nDone."
