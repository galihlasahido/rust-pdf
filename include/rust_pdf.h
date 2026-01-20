/*
 * rust-pdf C API
 *
 * This header provides a C-compatible interface to the rust-pdf library.
 * Link with: -lrust_pdf
 */

#ifndef RUST_PDF_H
#define RUST_PDF_H

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

/* Opaque handle to a PDF document */
typedef struct PdfHandle PdfHandle;

/*
 * Create a simple PDF with text.
 *
 * Parameters:
 *   text      - The text content (null-terminated UTF-8 string)
 *   font_size - Font size in points (e.g., 12.0, 24.0)
 *
 * Returns:
 *   A handle to the PDF document, or NULL on failure.
 *   Must be freed with pdf_free().
 */
PdfHandle* pdf_create_simple(const char* text, double font_size);

/*
 * Get the PDF data from a handle.
 *
 * Parameters:
 *   handle   - PDF handle from pdf_create_*
 *   out_data - Pointer to receive the data pointer
 *
 * Returns:
 *   The length of the data in bytes, or 0 on failure.
 *   The data pointer is valid until pdf_free() is called.
 */
size_t pdf_get_data(const PdfHandle* handle, const uint8_t** out_data);

/*
 * Save the PDF to a file.
 *
 * Parameters:
 *   handle - PDF handle from pdf_create_*
 *   path   - File path (null-terminated string)
 *
 * Returns:
 *   0 on success, -1 on failure.
 */
int pdf_save_to_file(const PdfHandle* handle, const char* path);

/*
 * Free a PDF handle.
 *
 * Parameters:
 *   handle - PDF handle to free (NULL is safely ignored)
 */
void pdf_free(PdfHandle* handle);

/*
 * Get the library version.
 *
 * Returns:
 *   A static string containing the version (e.g., "0.1.0").
 *   Do not free this string.
 */
const char* pdf_version(void);

#ifdef __cplusplus
}
#endif

#endif /* RUST_PDF_H */
