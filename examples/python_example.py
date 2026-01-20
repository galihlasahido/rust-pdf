#!/usr/bin/env python3
"""
Example: Using rust-pdf library from Python

Run with:
    # macOS
    DYLD_LIBRARY_PATH=../target/release python3 python_example.py

    # Linux
    LD_LIBRARY_PATH=../target/release python3 python_example.py
"""

import ctypes
from ctypes import c_char_p, c_double, c_void_p, c_size_t, c_int, POINTER, c_uint8
import os

def main():
    print("rust-pdf Python Example")
    print("=======================\n")

    # Determine library path
    script_dir = os.path.dirname(os.path.abspath(__file__))
    lib_dir = os.path.join(script_dir, "..", "target", "release")

    # Load the dynamic library
    if os.name == 'nt':  # Windows
        lib_path = os.path.join(lib_dir, "rust_pdf.dll")
    elif os.uname().sysname == 'Darwin':  # macOS
        lib_path = os.path.join(lib_dir, "librust_pdf.dylib")
    else:  # Linux
        lib_path = os.path.join(lib_dir, "librust_pdf.so")

    print(f"Loading library from: {lib_path}")
    lib = ctypes.CDLL(lib_path)

    # Define function signatures
    lib.pdf_version.restype = c_char_p
    lib.pdf_version.argtypes = []

    lib.pdf_create_simple.restype = c_void_p
    lib.pdf_create_simple.argtypes = [c_char_p, c_double]

    lib.pdf_get_data.restype = c_size_t
    lib.pdf_get_data.argtypes = [c_void_p, POINTER(POINTER(c_uint8))]

    lib.pdf_save_to_file.restype = c_int
    lib.pdf_save_to_file.argtypes = [c_void_p, c_char_p]

    lib.pdf_free.restype = None
    lib.pdf_free.argtypes = [c_void_p]

    # Get version
    version = lib.pdf_version().decode('utf-8')
    print(f"Library version: {version}\n")

    # Create PDF
    text = b"Hello from Python!\n\nThis PDF was created using rust-pdf via ctypes."
    print("Creating PDF...")
    pdf = lib.pdf_create_simple(text, 18.0)

    if not pdf:
        print("Error: Failed to create PDF")
        return

    # Get PDF size
    data_ptr = POINTER(c_uint8)()
    size = lib.pdf_get_data(pdf, ctypes.byref(data_ptr))
    print(f"PDF size: {size} bytes")

    # Save to file
    filename = b"hello_from_python.pdf"
    print(f"Saving to '{filename.decode()}'...")
    result = lib.pdf_save_to_file(pdf, filename)

    if result == 0:
        print("Success! PDF saved.")
    else:
        print("Error: Failed to save PDF")

    # Free resources
    lib.pdf_free(pdf)
    print("\nDone.")


if __name__ == "__main__":
    main()
