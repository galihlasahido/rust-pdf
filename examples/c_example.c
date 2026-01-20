/*
 * Example: Using rust-pdf library from C
 *
 * Compile with:
 *   clang -o pdf_example c_example.c -L../target/release -lrust_pdf -I../include
 *
 * Run with:
 *   DYLD_LIBRARY_PATH=../target/release ./pdf_example
 *
 * Or on Linux:
 *   gcc -o pdf_example c_example.c -L../target/release -lrust_pdf -I../include
 *   LD_LIBRARY_PATH=../target/release ./pdf_example
 */

#include <stdio.h>
#include <stdlib.h>
#include "rust_pdf.h"

int main(void) {
    printf("rust-pdf C Example\n");
    printf("==================\n\n");

    /* Print library version */
    printf("Library version: %s\n\n", pdf_version());

    /* Create a simple PDF */
    printf("Creating PDF with 'Hello from C!' text...\n");
    PdfHandle* pdf = pdf_create_simple("Hello from C!", 24.0);

    if (pdf == NULL) {
        fprintf(stderr, "Error: Failed to create PDF\n");
        return 1;
    }

    /* Get PDF data (for inspection or in-memory use) */
    const uint8_t* data;
    size_t len = pdf_get_data(pdf, &data);
    printf("PDF size: %zu bytes\n", len);

    /* Save to file */
    const char* filename = "hello_from_c.pdf";
    printf("Saving to '%s'...\n", filename);

    int result = pdf_save_to_file(pdf, filename);
    if (result == 0) {
        printf("Success! PDF saved to '%s'\n", filename);
    } else {
        fprintf(stderr, "Error: Failed to save PDF\n");
    }

    /* Free the handle */
    pdf_free(pdf);

    printf("\nDone.\n");
    return result;
}
