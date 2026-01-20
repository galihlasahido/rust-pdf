/**
 * Example: Using rust-pdf library from Node.js
 *
 * Prerequisites:
 *   npm install ffi-napi ref-napi
 *
 * Run with:
 *   # macOS
 *   DYLD_LIBRARY_PATH=../target/release node node_example.js
 *
 *   # Linux
 *   LD_LIBRARY_PATH=../target/release node node_example.js
 */

const ffi = require('ffi-napi');
const ref = require('ref-napi');
const path = require('path');

// Define types
const voidPtr = ref.refType(ref.types.void);
const uint8Ptr = ref.refType(ref.types.uint8);
const uint8PtrPtr = ref.refType(uint8Ptr);

// Determine library name based on platform
let libName;
if (process.platform === 'darwin') {
    libName = 'librust_pdf.dylib';
} else if (process.platform === 'win32') {
    libName = 'rust_pdf.dll';
} else {
    libName = 'librust_pdf.so';
}

// Load the library
const lib = ffi.Library(libName, {
    'pdf_version': ['string', []],
    'pdf_create_simple': [voidPtr, ['string', 'double']],
    'pdf_get_data': ['size_t', [voidPtr, uint8PtrPtr]],
    'pdf_save_to_file': ['int', [voidPtr, 'string']],
    'pdf_free': ['void', [voidPtr]]
});

console.log('rust-pdf Node.js Example');
console.log('========================\n');

// Get version
const version = lib.pdf_version();
console.log(`Library version: ${version}\n`);

// Create PDF
const text = 'Hello from Node.js!\n\nThis PDF was created using rust-pdf via ffi-napi.';
console.log('Creating PDF...');
const pdf = lib.pdf_create_simple(text, 18.0);

if (pdf.isNull()) {
    console.error('Error: Failed to create PDF');
    process.exit(1);
}

// Get PDF size
const dataPtr = ref.alloc(uint8Ptr);
const size = lib.pdf_get_data(pdf, dataPtr);
console.log(`PDF size: ${size} bytes`);

// Save to file
const filename = 'hello_from_node.pdf';
console.log(`Saving to '${filename}'...`);
const result = lib.pdf_save_to_file(pdf, filename);

if (result === 0) {
    console.log('Success! PDF saved.');
} else {
    console.error('Error: Failed to save PDF');
}

// Free resources
lib.pdf_free(pdf);
console.log('\nDone.');
