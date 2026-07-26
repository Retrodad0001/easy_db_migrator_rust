IF NOT EXISTS (SELECT * FROM sysobjects WHERE name='placeholder' AND xtype='U')
BEGIN
    CREATE TABLE placeholder
    (
        Id int IDENTITY(1,1) PRIMARY KEY
    )
END
