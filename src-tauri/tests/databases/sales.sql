-- Demo sales data, portable across PostgreSQL, MySQL and SQLite.
-- Used by the database tests, the Docker test servers and report examples.

DROP TABLE IF EXISTS orders;
DROP TABLE IF EXISTS products;
DROP TABLE IF EXISTS customers;

CREATE TABLE customers (
  id INTEGER PRIMARY KEY,
  name VARCHAR(100) NOT NULL,
  email VARCHAR(150) NOT NULL,
  region VARCHAR(50) NOT NULL,
  created_at DATE NOT NULL
);

CREATE TABLE products (
  id INTEGER PRIMARY KEY,
  name VARCHAR(100) NOT NULL,
  category VARCHAR(50) NOT NULL,
  price DECIMAL(10, 2) NOT NULL,
  active BOOLEAN NOT NULL
);

CREATE TABLE orders (
  id INTEGER PRIMARY KEY,
  customer_id INTEGER NOT NULL REFERENCES customers (id),
  product_id INTEGER NOT NULL REFERENCES products (id),
  quantity INTEGER NOT NULL,
  ordered_at DATE NOT NULL
);

INSERT INTO customers (id, name, email, region, created_at) VALUES
  (1, 'Acme Industrial', 'compras@acme.example', 'Norte', '2025-11-03'),
  (2, 'Café La Montaña', 'pedidos@lamontana.example', 'Centro', '2025-12-14'),
  (3, 'Grupo Delta', 'finanzas@delta.example', 'Sur', '2026-01-08'),
  (4, 'Taller Ruiz', 'contacto@tallerruiz.example', 'Centro', '2026-02-21'),
  (5, 'Northwind Retail', 'ops@northwind.example', 'Norte', '2026-03-02'),
  (6, 'Estudio Kairos', 'hola@kairos.example', 'Sur', '2026-04-17');

INSERT INTO products (id, name, category, price, active) VALUES
  (1, 'Laptop Pro 14', 'Equipos', 1299.99, TRUE),
  (2, 'Monitor 27"', 'Equipos', 349.50, TRUE),
  (3, 'Teclado mecánico', 'Accesorios', 89.90, TRUE),
  (4, 'Licencia anual', 'Software', 240.00, TRUE),
  (5, 'Soporte premium', 'Servicios', 600.00, FALSE);

INSERT INTO orders (id, customer_id, product_id, quantity, ordered_at) VALUES
  (1, 1, 1, 4, '2026-01-12'),
  (2, 1, 2, 4, '2026-01-12'),
  (3, 2, 3, 10, '2026-01-30'),
  (4, 3, 4, 25, '2026-02-05'),
  (5, 3, 1, 2, '2026-02-18'),
  (6, 4, 3, 3, '2026-03-01'),
  (7, 5, 2, 12, '2026-03-15'),
  (8, 5, 4, 12, '2026-03-15'),
  (9, 6, 5, 1, '2026-04-20'),
  (10, 2, 1, 1, '2026-05-02'),
  (11, 4, 4, 2, '2026-05-19'),
  (12, 1, 3, 15, '2026-06-07'),
  (13, 6, 2, 2, '2026-06-22'),
  (14, 3, 5, 1, '2026-07-09'),
  (15, 5, 1, 6, '2026-08-11'),
  (16, 2, 4, 1, '2026-08-30'),
  (17, 4, 1, 1, '2026-09-14'),
  (18, 6, 3, 5, '2026-10-01');
