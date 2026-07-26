IF NOT EXISTS (SELECT * FROM sysobjects WHERE name='goodtable' AND xtype='U')
BEGIN
    CREATE TABLE goodtable
    (
        Id int IDENTITY(1,1) PRIMARY KEY,
        Title nvarchar(40) NOT NULL
    )
END
