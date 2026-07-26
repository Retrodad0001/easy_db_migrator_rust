IF NOT EXISTS (SELECT * FROM sysobjects WHERE name='skippedtable' AND xtype='U')
BEGIN
    CREATE TABLE skippedtable
    (
        Id int IDENTITY(1,1) PRIMARY KEY,
        Title nvarchar(40) NOT NULL
    )
END
