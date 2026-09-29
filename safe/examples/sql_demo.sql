CREATE TABLE inventory(
  id INTEGER PRIMARY KEY,
  name TEXT NOT NULL,
  category TEXT,
  quantity INTEGER DEFAULT 0 CHECK(quantity >= 0),
  price REAL NOT NULL
);
INSERT INTO inventory(name,category,quantity,price) VALUES
  ('Notebook','Stationery',8,2.50),
  ('Pen','Stationery',12,1.25),
  ('Mug','Kitchen',4,7.50);

BEGIN;
UPDATE inventory SET quantity=quantity+2 WHERE id=1;
SAVEPOINT trial;
DELETE FROM inventory WHERE category='Kitchen';
ROLLBACK TO trial;
RELEASE trial;
COMMIT;

SELECT category,count(*) AS products,sum(quantity) AS units,
       sum(quantity*price) AS value
FROM inventory
GROUP BY category
ORDER BY category;
