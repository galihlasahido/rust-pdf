/*
Example: Using rust-pdf library from Go

Build and run with:

    # macOS
    CGO_LDFLAGS="-L../target/release -lrust_pdf" \
    DYLD_LIBRARY_PATH=../target/release \
    go run go_example.go

    # Linux
    CGO_LDFLAGS="-L../target/release -lrust_pdf" \
    LD_LIBRARY_PATH=../target/release \
    go run go_example.go
*/
package main

/*
#cgo LDFLAGS: -lrust_pdf
#include <stdlib.h>
#include <stdint.h>

// Function declarations (from rust_pdf.h)
typedef struct PdfHandle PdfHandle;
PdfHandle* pdf_create_simple(const char* text, double font_size);
size_t pdf_get_data(const PdfHandle* handle, const uint8_t** out_data);
int pdf_save_to_file(const PdfHandle* handle, const char* path);
void pdf_free(PdfHandle* handle);
const char* pdf_version(void);
*/
import "C"
import (
	"fmt"
	"unsafe"
)

func main() {
	fmt.Println("rust-pdf Go Example")
	fmt.Println("===================\n")

	// Get version
	version := C.GoString(C.pdf_version())
	fmt.Printf("Library version: %s\n\n", version)

	// Create PDF
	text := C.CString("Hello from Go!\n\nThis PDF was created using rust-pdf via cgo.")
	defer C.free(unsafe.Pointer(text))

	fmt.Println("Creating PDF...")
	pdf := C.pdf_create_simple(text, 18.0)

	if pdf == nil {
		fmt.Println("Error: Failed to create PDF")
		return
	}
	defer C.pdf_free(pdf)

	// Get PDF size
	var dataPtr *C.uint8_t
	size := C.pdf_get_data(pdf, &dataPtr)
	fmt.Printf("PDF size: %d bytes\n", size)

	// Save to file
	filename := C.CString("hello_from_go.pdf")
	defer C.free(unsafe.Pointer(filename))

	fmt.Println("Saving to 'hello_from_go.pdf'...")
	result := C.pdf_save_to_file(pdf, filename)

	if result == 0 {
		fmt.Println("Success! PDF saved.")
	} else {
		fmt.Println("Error: Failed to save PDF")
	}

	fmt.Println("\nDone.")
}
